//! Callback tool dispatcher — routes tool calls to an external client via JSON-RPC.
//!
//! When the agent invokes a callback tool, the dispatcher sends a `tool/execute`
//! request to the client over the RPC transport and awaits the response. This
//! enables Python/TypeScript SDKs to provide tool implementations.
//!
//! The tool list is **live**: it reads from a shared `Arc<RwLock<Vec<ToolDef>>>`
//! (the same one `tools/register` writes to), so tools registered after session
//! materialization become visible at the next turn boundary via
//! `poll_external_updates`.
//!
//! Per-session inline tools (from `session/create` `external_tools` param) are
//! held separately and take precedence on name collision with globals.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};

use base64::Engine;
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use async_trait::async_trait;
use meerkat_contracts::WireToolResultContent;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::agent::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::types::{ContentBlock, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry};

use crate::protocol::{RpcId, RpcOutboundAdmission, RpcOutboundPermit, RpcRequest, RpcResponse};

const CALLBACK_REQUEST_MAX_PARAMS_BYTES: usize = 16 * 1024 * 1024;
const CALLBACK_REQUEST_ENVELOPE_BYTES: usize = 4 * 1024;
const CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES: usize = 1024;
const CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES: usize = 256;
pub(crate) const CALLBACK_PROCESS_MAX_PENDING: usize = 64;
const CALLBACK_JOB_INITIAL_LEASE: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone)]
struct DetachedCallbackJobRuntime {
    realm_id: String,
    store: Arc<dyn meerkat::DetachedJobStore>,
    service: meerkat::DetachedJobService,
    blob_store: Arc<dyn meerkat_core::BlobStore>,
}

impl DetachedCallbackJobRuntime {
    fn new(
        realm_id: impl Into<String>,
        store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        Self {
            realm_id: realm_id.into(),
            service: meerkat::DetachedJobService::new(Arc::clone(&store)),
            store,
            blob_store,
        }
    }
}

#[derive(Clone)]
struct CallbackInvocationAdmission {
    semaphore: Arc<Semaphore>,
    max_pending: usize,
}

impl CallbackInvocationAdmission {
    fn production() -> Self {
        static PROCESS_ADMISSION: OnceLock<CallbackInvocationAdmission> = OnceLock::new();
        PROCESS_ADMISSION
            .get_or_init(|| Self::new(CALLBACK_PROCESS_MAX_PENDING))
            .clone()
    }

    fn new(max_pending: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_pending)),
            max_pending,
        }
    }

    fn try_acquire(&self) -> Result<OwnedSemaphorePermit, ToolError> {
        Arc::clone(&self.semaphore)
            .try_acquire_owned()
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "callback request admission rejected at the {}-request process limit: {error}",
                    self.max_pending
                ))
            })
    }
}

/// Inbound callback response plus the process-memory/count reservations that
/// remain live while the callback consumer parses and projects the payload.
/// Rejected handoffs carry a bounded synthesized error and no reservation.
pub struct CallbackResponseHandoff {
    response: RpcResponse,
    _memory_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    _count_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl CallbackResponseHandoff {
    pub(crate) fn admitted(
        response: RpcResponse,
        memory_permit: tokio::sync::OwnedSemaphorePermit,
        count_permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Self {
        Self {
            response,
            _memory_permit: Some(memory_permit),
            _count_permit: Some(count_permit),
        }
    }

    pub(crate) fn rejected(response: RpcResponse) -> Self {
        Self {
            response,
            _memory_permit: None,
            _count_permit: None,
        }
    }

    #[cfg(test)]
    fn unmetered_for_test(response: RpcResponse) -> Self {
        Self::rejected(response)
    }

    pub fn response(&self) -> &RpcResponse {
        &self.response
    }
}

/// Server→client callback request paired with the admitted response handoff.
/// The outbound permit remains attached through queueing and socket write;
/// callback invocation count admission is held separately by the dispatcher
/// until the response or timeout resolves.
pub struct CallbackRequestEnvelope {
    request: RpcRequest,
    response_tx: oneshot::Sender<CallbackResponseHandoff>,
    outbound_permit: Arc<RpcOutboundPermit>,
}

impl CallbackRequestEnvelope {
    fn new(
        request: RpcRequest,
        response_tx: oneshot::Sender<CallbackResponseHandoff>,
        outbound_permit: Arc<RpcOutboundPermit>,
    ) -> Self {
        Self {
            request,
            response_tx,
            outbound_permit,
        }
    }

    /// Split at the server transport boundary. The third tuple element must
    /// stay bound until `write_request` completes.
    pub(crate) fn into_parts(
        self,
    ) -> (
        RpcRequest,
        oneshot::Sender<CallbackResponseHandoff>,
        Arc<RpcOutboundPermit>,
    ) {
        (self.request, self.response_tx, self.outbound_permit)
    }
}

/// Timeout for waiting on a client tool response.
/// Public callback deadline. Hosts and transports add their own bounded
/// handoff margins (125s and 130s respectively).
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Clone)]
struct RegisteredCallbackTool {
    tool: ToolDef,
    execution: meerkat_core::ToolExecutionContract,
}

#[derive(Default)]
struct CallbackToolRegistryState {
    tools: Vec<RegisteredCallbackTool>,
}

/// One RPC connection's callback route: the channel its server writes
/// `tool/execute` requests to, the id space those requests are numbered in,
/// and the callback tools that connection registered.
///
/// A route belongs to exactly one connection (or to the process-default
/// stdio channel). Dispatchers capture the route they were built from, so a
/// session's callbacks always go to the connection that owns its route, and
/// fail typed (`tool_unavailable`) once that connection is gone instead of
/// borrowing another connection's channel or id space.
#[derive(Clone)]
pub struct CallbackRoute {
    tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    registry: Arc<CallbackToolRegistry>,
    route_id: CallbackRouteId,
}

/// The identity of one callback route: unique to this process and to the
/// route, kept by the route's clones. A detached callback job records its
/// submitting route's identity, so a route that acts on the job later can
/// tell whether it is that route; no route of another process (one after a
/// restart included) ever is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallbackRouteId(Arc<str>);

impl CallbackRouteId {
    fn mint() -> Self {
        static PROCESS: OnceLock<String> = OnceLock::new();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let process = PROCESS.get_or_init(|| uuid::Uuid::new_v4().to_string());
        Self(format!("{process}:{}", NEXT.fetch_add(1, Ordering::Relaxed)).into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl CallbackRoute {
    /// A fresh route over `tx` with its own id space and empty registry.
    pub fn new(tx: mpsc::Sender<CallbackRequestEnvelope>) -> Self {
        Self::from_parts(
            tx,
            Arc::new(AtomicU64::new(0)),
            Arc::new(CallbackToolRegistry::default()),
        )
    }

    /// A route over explicit parts (used by the process-default channel).
    pub fn from_parts(
        tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        registry: Arc<CallbackToolRegistry>,
    ) -> Self {
        Self {
            tx,
            id_counter,
            registry,
            route_id: CallbackRouteId::mint(),
        }
    }

    pub fn sender(&self) -> mpsc::Sender<CallbackRequestEnvelope> {
        self.tx.clone()
    }

    pub fn id_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.id_counter)
    }

    pub fn registry(&self) -> Arc<CallbackToolRegistry> {
        Arc::clone(&self.registry)
    }

    /// True once the owning connection stopped receiving callback requests.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Whether `other` is the same connection's route.
    pub fn same_connection(&self, other: &Self) -> bool {
        self.tx.same_channel(&other.tx)
    }

    /// This route's identity.
    pub(crate) fn route_id(&self) -> &CallbackRouteId {
        &self.route_id
    }
}

/// The callback route of one mob's members, held for the mob by the
/// connection that claimed it.
///
/// `mob/create` claims it for the creating connection, and
/// `mob/claim_callback_route` for the caller's. A mob no connection of this
/// process has claimed (one restored after a restart) holds none. A route
/// whose connection closed is released: no callback reaches it, a member's
/// call fails `tool_unavailable` with
/// [`meerkat_core::ToolUnavailableReason::CallbackRouteUnclaimed`], and
/// another connection may claim the slot. The released route's tool
/// definitions stay advertised until then, so a member's call of one fails
/// with that reason instead of as an unknown tool. A claim never takes the
/// route from a connection that is still open.
///
/// Every dispatcher built on the slot reads it at each use, so a claim
/// reaches the mob's live members and the ones built later alike.
#[derive(Default)]
pub struct MobCallbackRouteSlot {
    route: StdRwLock<Option<CallbackRoute>>,
    generation: AtomicU64,
}

/// Why a claim of a mob's callback route was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MobCallbackRouteClaimRefused {
    /// Another connection that is still open holds the route.
    #[error("another open connection holds this mob's callback route")]
    HeldByOpenConnection,
}

impl MobCallbackRouteSlot {
    /// A slot no connection holds.
    pub fn unclaimed() -> Self {
        Self::default()
    }

    /// A slot `route`'s connection holds.
    pub fn claimed_by(route: CallbackRoute) -> Self {
        Self {
            route: StdRwLock::new(Some(route)),
            generation: AtomicU64::new(0),
        }
    }

    /// Claim the slot for `route`'s connection. A connection that already
    /// holds it keeps it; a connection that is still open is never displaced.
    pub fn claim(&self, route: &CallbackRoute) -> Result<(), MobCallbackRouteClaimRefused> {
        let mut held = self
            .route
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match held.as_ref() {
            Some(current) if current.same_connection(route) => Ok(()),
            Some(current) if !current.is_closed() => {
                Err(MobCallbackRouteClaimRefused::HeldByOpenConnection)
            }
            _ => {
                *held = Some(route.clone());
                self.generation.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
        }
    }

    /// The route whose tools the slot advertises: the one claimed last,
    /// whether or not its connection is still open.
    fn advertised_route(&self) -> Option<CallbackRoute> {
        self.route
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The route a callback reaches now: the claimed one, while its
    /// connection is open.
    fn live_route(&self) -> Option<CallbackRoute> {
        self.advertised_route().filter(|route| !route.is_closed())
    }

    /// Advances at every claim that changes the holder.
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// Where a callback dispatcher sends its calls.
enum CallbackRouteSource {
    /// The route it was built on: a session's connection, or the
    /// process-default channel.
    Fixed(CallbackRoute),
    /// A mob's claimable route slot, read at every use.
    MobSlot(Arc<MobCallbackRouteSlot>),
}

/// Live callback-tool registry whose semantic mutation and epoch advancement
/// share one lock/authority boundary.
pub struct CallbackToolRegistry {
    state: Arc<StdRwLock<CallbackToolRegistryState>>,
    generation: Arc<AtomicU64>,
}

impl Default for CallbackToolRegistry {
    fn default() -> Self {
        Self {
            state: Arc::new(StdRwLock::new(CallbackToolRegistryState::default())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl CallbackToolRegistry {
    pub(crate) fn replace_or_add(&self, replacements: Vec<ToolDef>) -> Result<usize, ()> {
        self.replace_or_add_with_contracts(
            replacements
                .into_iter()
                .map(|tool| (tool, meerkat_core::ToolExecutionContract::default()))
                .collect(),
        )
    }

    pub(crate) fn replace_or_add_with_contracts(
        &self,
        replacements: Vec<(ToolDef, meerkat_core::ToolExecutionContract)>,
    ) -> Result<usize, ()> {
        let mut state = self.state.write().map_err(|_| ())?;
        let count = replacements.len();
        for (tool, execution) in replacements {
            let replacement = RegisteredCallbackTool { tool, execution };
            if let Some(existing) = state
                .tools
                .iter_mut()
                .find(|registered| registered.tool.name == replacement.tool.name)
            {
                *existing = replacement;
            } else {
                state.tools.push(replacement);
            }
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(count)
    }

    pub fn snapshot(&self) -> Vec<ToolDef> {
        self.state
            .read()
            .map(|state| {
                state
                    .tools
                    .iter()
                    .map(|registered| registered.tool.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn from_test_parts(tools: Arc<StdRwLock<Vec<ToolDef>>>, generation: Arc<AtomicU64>) -> Self {
        let tools = tools
            .read()
            .map(|tools| {
                tools
                    .iter()
                    .cloned()
                    .map(|tool| RegisteredCallbackTool {
                        tool,
                        execution: meerkat_core::ToolExecutionContract::default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            state: Arc::new(StdRwLock::new(CallbackToolRegistryState { tools })),
            generation,
        }
    }
}

/// Dispatches tool calls to an external client via the RPC callback protocol.
///
/// Holds a shared reference to the global registered-tools list so that tools
/// registered via `tools/register` after session creation are picked up
/// dynamically at each turn boundary (via `poll_external_updates`).
///
/// Per-session `inline_tools` (from `session/create` `external_tools` param) are
/// static and take precedence on name collision with globals. Most sessions have
/// no inline tools (the field is empty).
pub struct CallbackToolDispatcher {
    /// The route whose live registry the tools come from and whose channel
    /// and id space the calls use.
    route: CallbackRouteSource,
    /// Per-session inline tools (static, usually empty). Win on name collision.
    inline_tools: Vec<ToolDef>,
    /// Inline tool names cached for fast collision lookup.
    inline_names: HashSet<String>,
    detached_jobs: Option<DetachedCallbackJobRuntime>,
    /// Snapshot of global tool names from the last `poll_external_updates` (or creation).
    /// Used to detect additions/removals between polls.
    last_seen_globals: StdRwLock<HashSet<String>>,
    /// Runs between a detached job's route check and the send to that route
    /// (reconcile and cancel), so a test can hand the mob slot over there.
    #[cfg(test)]
    check_send_seam: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl CallbackToolDispatcher {
    /// Create a callback dispatcher backed by the live registered-tools list.
    ///
    /// `inline_tools` are per-session static tools (from `session/create`
    /// `external_tools` param). Pass empty vec for most sessions.
    #[cfg(test)]
    fn new(
        registered_tools: Arc<StdRwLock<Vec<ToolDef>>>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        Self::new_with_generation(
            registered_tools,
            Arc::new(AtomicU64::new(0)),
            callback_tx,
            id_counter,
            inline_tools,
        )
    }

    /// Create a callback dispatcher bound to the production registry epoch.
    #[cfg(test)]
    fn new_with_generation(
        registered_tools: Arc<StdRwLock<Vec<ToolDef>>>,
        registered_tools_generation: Arc<AtomicU64>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        Self::from_registry(
            Arc::new(CallbackToolRegistry::from_test_parts(
                registered_tools,
                registered_tools_generation,
            )),
            callback_tx,
            id_counter,
            inline_tools,
        )
    }

    /// Create a callback dispatcher bound to the inseparable production
    /// registry mutation/epoch authority.
    pub fn from_registry(
        registered_tools: Arc<CallbackToolRegistry>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
    ) -> Self {
        Self::from_source(
            CallbackRouteSource::Fixed(CallbackRoute::from_parts(
                callback_tx,
                id_counter,
                registered_tools,
            )),
            inline_tools,
        )
    }

    /// Create a callback dispatcher for a mob's members, bound to the mob's
    /// claimable route slot: it follows every later claim of the slot.
    pub fn from_mob_slot(slot: Arc<MobCallbackRouteSlot>, inline_tools: Vec<ToolDef>) -> Self {
        Self::from_source(CallbackRouteSource::MobSlot(slot), inline_tools)
    }

    /// [`Self::from_mob_slot`] that can also own detached callback jobs.
    pub fn from_mob_slot_with_job_runtime(
        slot: Arc<MobCallbackRouteSlot>,
        inline_tools: Vec<ToolDef>,
        realm_id: impl Into<String>,
        job_store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        let mut dispatcher = Self::from_mob_slot(slot, inline_tools);
        dispatcher.detached_jobs = Some(DetachedCallbackJobRuntime::new(
            realm_id, job_store, blob_store,
        ));
        dispatcher
    }

    fn from_source(route: CallbackRouteSource, inline_tools: Vec<ToolDef>) -> Self {
        let inline_names: HashSet<String> =
            inline_tools.iter().map(|t| t.name.to_string()).collect();
        let mut dispatcher = Self {
            route,
            inline_tools,
            inline_names,
            detached_jobs: None,
            last_seen_globals: StdRwLock::new(HashSet::new()),
            #[cfg(test)]
            check_send_seam: None,
        };
        let last_seen_globals = dispatcher.current_global_names();
        dispatcher.last_seen_globals = StdRwLock::new(last_seen_globals);
        dispatcher
    }

    /// The registry whose tools this dispatcher advertises.
    fn advertised_registry(&self) -> Option<Arc<CallbackToolRegistry>> {
        match &self.route {
            CallbackRouteSource::Fixed(route) => Some(route.registry()),
            CallbackRouteSource::MobSlot(slot) => {
                slot.advertised_route().map(|route| route.registry())
            }
        }
    }

    /// The route a call of `tool_name` goes to now. A fixed route is used
    /// as is (a closed one fails at the send); a mob slot without an open
    /// claimed route fails here, typed.
    fn call_route(&self, tool_name: &str) -> Result<CallbackRoute, ToolError> {
        match &self.route {
            CallbackRouteSource::Fixed(route) => Ok(route.clone()),
            CallbackRouteSource::MobSlot(slot) => slot.live_route().ok_or_else(|| {
                ToolError::unavailable(
                    tool_name,
                    meerkat_core::ToolUnavailableReason::CallbackRouteUnclaimed,
                )
            }),
        }
    }

    /// Why a call whose route's connection closed mid-call is unavailable:
    /// a mob slot is released with its connection.
    fn route_gone_reason(&self) -> meerkat_core::ToolUnavailableReason {
        match &self.route {
            CallbackRouteSource::Fixed(_) => {
                meerkat_core::ToolUnavailableReason::NotCurrentlyCallable
            }
            CallbackRouteSource::MobSlot(_) => {
                meerkat_core::ToolUnavailableReason::CallbackRouteUnclaimed
            }
        }
    }

    /// Run the test seam between a detached job's route check and the send to
    /// that route, if one is set.
    fn run_check_send_seam(&self) {
        #[cfg(test)]
        if let Some(seam) = &self.check_send_seam {
            seam();
        }
    }

    #[cfg(test)]
    fn with_check_send_seam(mut self, seam: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.check_send_seam = Some(seam);
        self
    }

    /// The route detached callback jobs are started and reconciled on.
    fn job_route(&self) -> Result<CallbackRoute, String> {
        match &self.route {
            CallbackRouteSource::Fixed(route) => Ok(route.clone()),
            CallbackRouteSource::MobSlot(slot) => slot.live_route().ok_or_else(|| {
                meerkat_core::ToolUnavailableReason::CallbackRouteUnclaimed.to_string()
            }),
        }
    }

    /// Create a callback dispatcher bound to one connection's route.
    pub fn from_route(route: &CallbackRoute, inline_tools: Vec<ToolDef>) -> Self {
        Self::from_source(CallbackRouteSource::Fixed(route.clone()), inline_tools)
    }

    /// Create a route-bound dispatcher that can also own detached callback jobs.
    pub fn from_route_with_job_runtime(
        route: &CallbackRoute,
        inline_tools: Vec<ToolDef>,
        realm_id: impl Into<String>,
        job_store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        let mut dispatcher = Self::from_route(route, inline_tools);
        dispatcher.detached_jobs = Some(DetachedCallbackJobRuntime::new(
            realm_id, job_store, blob_store,
        ));
        dispatcher
    }

    pub fn from_registry_with_job_runtime(
        registered_tools: Arc<CallbackToolRegistry>,
        callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
        id_counter: Arc<AtomicU64>,
        inline_tools: Vec<ToolDef>,
        realm_id: impl Into<String>,
        job_store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> Self {
        let mut dispatcher =
            Self::from_registry(registered_tools, callback_tx, id_counter, inline_tools);
        dispatcher.detached_jobs = Some(DetachedCallbackJobRuntime::new(
            realm_id, job_store, blob_store,
        ));
        dispatcher
    }

    fn next_id(route: &CallbackRoute) -> RpcId {
        let n = route.id_counter.fetch_add(1, Ordering::Relaxed);
        RpcId::Str(format!("srv-{n}"))
    }

    /// Current global tool names excluding inline collisions.
    fn current_global_names(&self) -> HashSet<String> {
        let Some(registry) = self.advertised_registry() else {
            return HashSet::new();
        };
        registry
            .state
            .read()
            .map(|state| {
                state
                    .tools
                    .iter()
                    .filter(|registered| !self.inline_names.contains(registered.tool.name.as_str()))
                    .map(|registered| registered.tool.name.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_known(&self, name: &str) -> bool {
        if self.inline_names.contains(name) {
            return true;
        }
        self.advertised_registry().is_some_and(|registry| {
            registry
                .state
                .read()
                .map(|state| {
                    state
                        .tools
                        .iter()
                        .any(|registered| registered.tool.name == name)
                })
                .unwrap_or(false)
        })
    }

    /// Send one `tool/execute` request to the route that registers the
    /// tool and await its result. The request carries `dedupe_key`, the
    /// call's dedupe key, when there is one, and omits the field otherwise.
    async fn send_tool_execute(
        &self,
        call: ToolCallView<'_>,
        dedupe_key: Option<meerkat_core::ToolCallDedupeKey>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.is_known(call.name) {
            if let CallbackRouteSource::MobSlot(slot) = &self.route
                && slot.live_route().is_none()
            {
                return Err(ToolError::unavailable(
                    call.name,
                    meerkat_core::ToolUnavailableReason::CallbackRouteUnclaimed,
                ));
            }
            return Err(ToolError::not_found(call.name));
        }

        // This permit remains local across queueing and the response timeout,
        // providing a process-global hard cap on both callback dispatch tasks
        // and entries in the server's pending-callback map.
        let _invocation_permit = CallbackInvocationAdmission::production().try_acquire()?;
        if call.id.len() > CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES {
            return Err(ToolError::invalid_arguments(
                call.name,
                format!(
                    "tool_use_id exceeds the {CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES}-byte callback limit"
                ),
            ));
        }
        if call.name.len() > CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES {
            return Err(ToolError::invalid_arguments(
                call.name,
                format!(
                    "tool name exceeds the {CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES}-byte callback limit"
                ),
            ));
        }
        if call.args.get().len() > CALLBACK_REQUEST_MAX_PARAMS_BYTES {
            return Err(ToolError::invalid_arguments(
                call.name,
                format!(
                    "tool arguments exceed the {CALLBACK_REQUEST_MAX_PARAMS_BYTES}-byte callback limit"
                ),
            ));
        }

        #[derive(Serialize)]
        struct CallbackParams<'a> {
            tool_use_id: &'a str,
            name: &'a str,
            arguments: &'a RawValue,
            #[serde(skip_serializing_if = "Option::is_none")]
            dedupe_key: Option<meerkat_core::ToolCallDedupeKey>,
        }

        let route = self.call_route(call.name)?;
        let request_id = Self::next_id(&route);
        let params = CallbackParams {
            tool_use_id: call.id,
            name: call.name,
            arguments: call.args,
            dedupe_key,
        };
        let (params_raw, outbound_permit) = RpcOutboundAdmission::production()
            .admit_json(
                &params,
                CALLBACK_REQUEST_MAX_PARAMS_BYTES,
                CALLBACK_REQUEST_ENVELOPE_BYTES,
            )
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "callback request rejected by outbound admission: {error}"
                ))
            })?;

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(request_id),
            method: "tool/execute".to_string(),
            params: Some(params_raw),
        };

        let (response_tx, response_rx) = oneshot::channel();

        route
            .tx
            .send(CallbackRequestEnvelope::new(
                request,
                response_tx,
                outbound_permit,
            ))
            .await
            // The route's owning connection is gone: the tool is unavailable
            // to this session. It never falls back to another connection.
            .map_err(|_| ToolError::unavailable(call.name, self.route_gone_reason()))?;

        let response_handoff = tokio::time::timeout(CALLBACK_TIMEOUT, response_rx)
            .await
            .map_err(|_| callback_timeout_error(call.name))?
            // The owning connection closed and failed its pending callbacks.
            .map_err(|_| ToolError::unavailable(call.name, self.route_gone_reason()))?;

        let response = response_handoff.response();

        if let Some(error) = response.error.as_ref() {
            let message = error.message.clone();
            drop(response_handoff);
            return Err(ToolError::execution_failed(message));
        }

        let result_raw = response.result.as_deref().ok_or_else(|| {
            ToolError::execution_failed("Callback response missing result".to_string())
        })?;

        #[derive(serde::Deserialize)]
        struct CallbackResult {
            #[serde(default)]
            content: Option<WireToolResultContent>,
            #[serde(default)]
            is_error: bool,
        }

        let parsed: CallbackResult = serde_json::from_str(result_raw.get()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse callback result: {e}"))
        })?;
        let result = match parsed.content {
            None => ToolResult::new(call.id.to_string(), String::new(), parsed.is_error),
            Some(WireToolResultContent::Text(text)) => {
                ToolResult::new(call.id.to_string(), text, parsed.is_error)
            }
            Some(WireToolResultContent::Blocks(blocks)) => {
                let content = blocks
                    .into_iter()
                    .map(ContentBlock::try_from)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        ToolError::execution_failed(format!(
                            "Failed to convert callback result content: {error}"
                        ))
                    })?;
                ToolResult::with_blocks(call.id.to_string(), content, parsed.is_error)
            }
        };
        let outcome = result.into();
        drop(response_handoff);
        Ok(outcome)
    }

    async fn submit_detached(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let runtime = self.detached_jobs.clone().ok_or_else(|| {
            ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            )
        })?;
        // The route first: a call no open connection can take writes nothing
        // durable, so no later holder of the route can run it.
        let route = self.call_route(call.name)?;
        // The job records the route it is submitted on and the declaration
        // of the tool that route registers: the route that runs it.
        let tool_binding =
            registered_tool_binding_in(&route.registry(), call.name).ok_or_else(|| {
                ToolError::unavailable(
                    call.name,
                    meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                )
            })?;
        let (spec, arguments) = detached_callback_job_spec(&runtime.realm_id, call, context, plan)?;
        // The blob contract carries base64-encoded bytes: store the base64 of
        // the specification record, which keeps the exact argument bytes the
        // call carried and the declaration of the tool that took it.
        let record = serde_json::to_vec(&CallbackSpecification {
            arguments: call.args.to_owned(),
            tool_binding_sha256: tool_binding,
            submitted_route: route.route_id().as_str().to_string(),
        })
        .map_err(|error| {
            ToolError::execution_failed(format!(
                "failed to encode detached callback specification: {error}"
            ))
        })?;
        let specification = runtime
            .blob_store
            .put_artifact(
                CALLBACK_SPECIFICATION_MEDIA_TYPE,
                &base64::engine::general_purpose::STANDARD.encode(record),
            )
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "failed to persist detached callback specification: {error}"
                ))
            })?;
        let spec = spec.with_runner_specification_ref(
            meerkat::RunnerSpecificationRef::new(specification.blob_id.to_string())
                .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        );
        let receipt = match runtime.service.submit(spec.clone()).await {
            Ok(receipt) => receipt,
            Err(meerkat::DetachedJobError::SubmissionConflict) => {
                resubmit_under_recorded_arguments(&runtime, spec, &arguments).await?
            }
            Err(error) => return Err(ToolError::execution_failed(error.to_string())),
        };
        let wire_receipt = meerkat::project_job_receipt(receipt.clone());

        let callback_tx = route.sender();
        let id_counter = route.id_counter();
        let acting_route = route.route_id().clone();
        tokio::spawn(async move {
            if let Err(error) = start_detached_callback_attempt(
                runtime,
                callback_tx,
                id_counter,
                receipt.job_id,
                ActingRoute {
                    route_id: acting_route,
                    tool_binding: Some(tool_binding),
                },
            )
            .await
            {
                tracing::warn!(%error, "detached callback attempt start did not complete");
            }
        });

        let content = serde_json::to_string(&wire_receipt).map_err(|error| {
            ToolError::execution_failed(format!("failed to encode detached job receipt: {error}"))
        })?;
        Ok(ToolResult::new(call.id.to_string(), content, false).into())
    }

    /// Reconcile host-visible attempts from committed authority without
    /// claiming, retrying, or otherwise advancing attempt/fence state.
    pub async fn reconcile_detached_jobs(&self) -> Result<(), String> {
        let Some(runtime) = self.detached_jobs.clone() else {
            return Ok(());
        };
        let jobs = runtime.store.list_all(10_000).await.map_err(|error| {
            format!("failed to enumerate detached callbacks for reconciliation: {error}")
        })?;
        let active = jobs
            .into_iter()
            .filter(|job| {
                job.spec.realm_id == runtime.realm_id
                    && self.owns_detached_callback_spec(&job.spec)
                    && matches!(
                        job.machine_state.lifecycle_phase,
                        meerkat::JobPhase::Running | meerkat::JobPhase::WaitingExternal
                    )
            })
            .collect::<Vec<_>>();
        // One route for the pass: every active job is checked against the
        // route its reconcile request then goes to.
        let route = if active.is_empty() {
            None
        } else {
            Some(self.job_route()?)
        };
        let mut attempts = Vec::new();
        if let Some(route) = route.as_ref() {
            for job in active {
                if !registry_owns_detached_callback_spec(&route.registry(), &job.spec) {
                    continue;
                }
                match Self::route_may_act_on(&runtime, &job.spec, route).await {
                    Ok(true) => {}
                    Ok(false) => {
                        mark_callback_needs_attention(
                            &runtime,
                            &job.job_id,
                            CALLBACK_BINDING_CHANGED,
                        )
                        .await?;
                        continue;
                    }
                    Err(SpecificationReadError::Unreadable(error)) => {
                        tracing::warn!(
                            job_id = %job.job_id,
                            %error,
                            "active detached callback has an unreadable specification"
                        );
                        mark_callback_needs_attention(
                            &runtime,
                            &job.job_id,
                            CALLBACK_SPECIFICATION_UNREADABLE,
                        )
                        .await?;
                        continue;
                    }
                    Err(SpecificationReadError::Unavailable(error)) => {
                        tracing::warn!(
                            job_id = %job.job_id,
                            %error,
                            "active detached callback left unreconciled"
                        );
                        continue;
                    }
                }
                let Some(attempt_id) = job.machine_state.current_attempt_id.as_deref() else {
                    return Err(format!(
                        "active detached callback {} has no committed attempt id",
                        job.job_id
                    ));
                };
                let Some(runner_handle) = job.machine_state.runner_handle.as_deref() else {
                    return Err(format!(
                        "active detached callback {} has no committed runner handle",
                        job.job_id
                    ));
                };
                let Some(lease_expires_at_ms) = job.machine_state.lease_expires_at_ms else {
                    return Err(format!(
                        "active detached callback {} has no committed lease",
                        job.job_id
                    ));
                };
                attempts.push(meerkat_contracts::CallbackJobReconcileAttempt {
                    authority: meerkat_contracts::JobAttemptAuthority {
                        job_id: job.job_id.to_string(),
                        attempt_id: attempt_id.to_string(),
                        fence: job.machine_state.current_fence,
                    },
                    runner: meerkat_contracts::JobRunner {
                        name: job.spec.runner.name().to_string(),
                        version: job.spec.runner.version().to_string(),
                    },
                    restart_class: wire_restart_class(job.spec.restart_class),
                    runner_handle: runner_handle.to_string(),
                    checkpoint_ref: job
                        .machine_state
                        .checkpoint_ref
                        .as_ref()
                        .map(ToString::to_string),
                    lease_expires_at_ms,
                });
            }
        }
        if !attempts.is_empty() {
            let offered_attempts = attempts.clone();
            let offered = offered_attempts
                .iter()
                .map(|attempt| attempt.authority.clone())
                .collect::<Vec<_>>();
            let Some(route) = route else {
                return Err(
                    "detached callback reconciliation offered attempts it checked against no route"
                        .to_string(),
                );
            };
            self.run_check_send_seam();
            let result: meerkat_contracts::CallbackJobReconcileResult = send_callback_request(
                route.sender(),
                route.id_counter(),
                "callback/job/reconcile",
                &meerkat_contracts::CallbackJobReconcileParams { attempts },
            )
            .await?;
            if result
                .live_attempts
                .iter()
                .any(|live| !offered.contains(live))
            {
                return Err(
                    "callback/job/reconcile returned authority that was not offered".to_string(),
                );
            }
            let now_ms = unix_time_ms()?;
            for attempt in offered_attempts.iter().filter(|attempt| {
                attempt.lease_expires_at_ms < now_ms
                    && !result.live_attempts.contains(&attempt.authority)
            }) {
                let job_id =
                    meerkat::JobId::new(&attempt.authority.job_id).map_err(|e| e.to_string())?;
                let write = meerkat::AttemptWriteAuthority {
                    attempt_id: meerkat::AttemptId::new(&attempt.authority.attempt_id)
                        .map_err(|e| e.to_string())?,
                    fence: meerkat::FenceToken::new(attempt.authority.fence),
                };
                match runtime
                    .service
                    .observe_lease_expired(&job_id, write, now_ms)
                    .await
                {
                    Ok(_) => {}
                    Err(meerkat::DetachedJobError::StaleRevision { .. })
                    | Err(meerkat::DetachedJobError::InvalidTransition { .. }) => {
                        continue;
                    }
                    Err(error) => return Err(error.to_string()),
                }
                match attempt.restart_class {
                    meerkat_contracts::JobRestartClass::NonResumable => {
                        runtime
                            .service
                            .classify_worker_loss(&job_id, now_ms)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    meerkat_contracts::JobRestartClass::CheckpointResumable
                        if attempt.checkpoint_ref.is_none() =>
                    {
                        runtime
                            .service
                            .mark_needs_attention(
                                &job_id,
                                now_ms,
                                meerkat::JobFailureCode::new(
                                    "checkpoint_resume_missing_checkpoint",
                                )
                                .map_err(|error| error.to_string())?,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    meerkat_contracts::JobRestartClass::Adoptable
                    | meerkat_contracts::JobRestartClass::CheckpointResumable
                    | meerkat_contracts::JobRestartClass::Replayable => {
                        runtime
                            .service
                            .schedule_retry(&job_id, now_ms)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        self.start_due_detached_jobs().await
    }

    /// Start queued or due-retry callback jobs owned by the current callback
    /// registration. Each start still commits an explicit machine claim.
    pub async fn start_due_detached_jobs(&self) -> Result<(), String> {
        let Some(runtime) = self.detached_jobs.clone() else {
            return Ok(());
        };
        let now_ms = unix_time_ms()?;
        let jobs = runtime.store.list_all(10_000).await.map_err(|error| {
            format!("failed to enumerate detached callbacks for runnable work: {error}")
        })?;
        for job in jobs.into_iter().filter(|job| {
            job.spec.realm_id == runtime.realm_id
                && self.owns_detached_callback_spec(&job.spec)
                && (job.machine_state.lifecycle_phase == meerkat::JobPhase::Queued
                    || (job.machine_state.lifecycle_phase == meerkat::JobPhase::RetryScheduled
                        && job
                            .machine_state
                            .retry_due_at_ms
                            .is_some_and(|due_at_ms| now_ms >= due_at_ms)))
        }) {
            let runtime = runtime.clone();
            let route = self.job_route()?;
            if !registry_owns_detached_callback_spec(&route.registry(), &job.spec) {
                continue;
            }
            let callback_tx = route.sender();
            let id_counter = route.id_counter();
            let acting = ActingRoute {
                route_id: route.route_id().clone(),
                tool_binding: registered_tool_binding_in(&route.registry(), job.spec.tool.name()),
            };
            tokio::spawn(async move {
                if let Err(error) = start_detached_callback_attempt(
                    runtime,
                    callback_tx,
                    id_counter,
                    job.job_id,
                    acting,
                )
                .await
                {
                    tracing::warn!(%error, "runnable detached callback start did not complete");
                }
            });
        }
        Ok(())
    }

    fn owns_detached_callback_spec(&self, spec: &meerkat::JobSpec) -> bool {
        self.advertised_registry()
            .is_some_and(|registry| registry_owns_detached_callback_spec(&registry, spec))
    }

    /// Whether `route` may act on the job `spec` describes
    /// ([`RetainedCallbackSpecification::admits`]): it is the route that
    /// submitted the job, or its tool declares what the job recorded. A job
    /// whose specification recorded neither (written before 0.9.3) is never
    /// admitted. `Err` when its specification cannot be read.
    async fn route_may_act_on(
        runtime: &DetachedCallbackJobRuntime,
        spec: &meerkat::JobSpec,
        route: &CallbackRoute,
    ) -> Result<bool, SpecificationReadError> {
        let Some(reference) = spec.runner_specification_ref.as_ref() else {
            return Ok(false);
        };
        let specification = read_callback_specification(runtime, reference).await?;
        Ok(specification.admits(&ActingRoute {
            route_id: route.route_id().clone(),
            tool_binding: registered_tool_binding_in(&route.registry(), spec.tool.name()),
        }))
    }

    /// Deliver a committed cancellation request to the host using the exact
    /// current attempt authority. This never claims work or advances a fence.
    pub async fn cancel_detached_job(&self, job_id: &meerkat::JobId) -> Result<(), String> {
        let Some(runtime) = self.detached_jobs.as_ref() else {
            return Ok(());
        };
        let Some(job) = runtime
            .store
            .get(job_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err(format!("detached callback job {job_id} does not exist"));
        };
        if job.spec.realm_id != runtime.realm_id
            || !self.owns_detached_callback_spec(&job.spec)
            || !matches!(
                job.machine_state.lifecycle_phase,
                meerkat::JobPhase::Running | meerkat::JobPhase::WaitingExternal
            )
            || !job.machine_state.cancel_requested
        {
            return Ok(());
        }
        // One route for the check and the send.
        let route = self.job_route()?;
        if !registry_owns_detached_callback_spec(&route.registry(), &job.spec) {
            return Ok(());
        }
        match Self::route_may_act_on(runtime, &job.spec, &route).await {
            Ok(true) => {}
            Ok(false) => {
                return mark_callback_needs_attention(runtime, job_id, CALLBACK_BINDING_CHANGED)
                    .await;
            }
            Err(SpecificationReadError::Unreadable(_)) => {
                return mark_callback_needs_attention(
                    runtime,
                    job_id,
                    CALLBACK_SPECIFICATION_UNREADABLE,
                )
                .await;
            }
            Err(error) => return Err(error.to_string()),
        }
        let attempt_id = job
            .machine_state
            .current_attempt_id
            .as_deref()
            .ok_or_else(|| format!("active detached callback {job_id} has no attempt id"))?;
        self.run_check_send_seam();
        let result: meerkat_contracts::CallbackJobCancelResult = send_callback_request(
            route.sender(),
            route.id_counter(),
            "callback/job/cancel",
            &meerkat_contracts::CallbackJobCancelParams {
                authority: meerkat_contracts::JobAttemptAuthority {
                    job_id: job_id.to_string(),
                    attempt_id: attempt_id.to_string(),
                    fence: job.machine_state.current_fence,
                },
            },
        )
        .await?;
        if !result.accepted {
            return Err(format!("callback/job/cancel rejected job {job_id}"));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentToolDispatcher for CallbackToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut result: Vec<Arc<ToolDef>> = self
            .inline_tools
            .iter()
            .map(|t| Arc::new(t.clone()))
            .collect();
        let registry = self.advertised_registry();
        if let Some(Ok(global)) = registry.as_ref().map(|registry| registry.state.read()) {
            for registered in &global.tools {
                if !self.inline_names.contains(registered.tool.name.as_str()) {
                    result.push(Arc::new(registered.tool.clone()));
                }
            }
        }
        result.into()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut result: Vec<ToolCatalogEntry> = self
            .inline_tools
            .iter()
            .map(|tool| {
                ToolCatalogEntry::session_inline(Arc::new(tool.clone()), true)
                    .with_execution_contract(meerkat_core::ToolExecutionContract::default())
            })
            .collect();
        let registry = self.advertised_registry();
        if let Some(Ok(global)) = registry.as_ref().map(|registry| registry.state.read()) {
            for registered in &global.tools {
                if !self.inline_names.contains(registered.tool.name.as_str()) {
                    result.push(callback_catalog_entry(
                        registered.tool.clone(),
                        registered.execution.clone(),
                    ));
                }
            }
        }
        result.into()
    }

    fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
        self.advertised_registry()
            .map_or(0, |registry| registry.generation())
    }

    /// A mob slot's claims rebind every tool it advertises: the slot's claim
    /// generation is part of the binding, beside the route registry's own.
    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        let fingerprint = meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
            .with_live_authority(0, self.execution_binding_epoch(tool_name));
        Ok(match &self.route {
            CallbackRouteSource::Fixed(_) => fingerprint,
            CallbackRouteSource::MobSlot(slot) => {
                fingerprint.with_live_authority(1, slot.generation())
            }
        })
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        let resolved_context =
            resolution_context.with_deadline(meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::ToolInternal,
                CALLBACK_TIMEOUT,
            ))?;
        entry
            .execution
            .resolve_default(resolved_context.deadlines().clone())
            .map_err(Into::into)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.send_tool_execute(call, None).await
    }

    /// A `tool/execute` request that carries the call's dedupe key when an
    /// agent-loop round dispatched it.
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.send_tool_execute(call, context.tool_call_dedupe_key(call))
            .await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        match plan.mode() {
            meerkat_core::ToolExecutionMode::Fast => {
                self.dispatch_with_context(call, context).await
            }
            meerkat_core::ToolExecutionMode::Detached => {
                self.submit_detached(call, context, plan).await
            }
            meerkat_core::ToolExecutionMode::Streaming => Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            )),
        }
    }

    /// Detect global tools added/removed since last poll and emit deltas.
    /// Inline tools are static and never produce deltas.
    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        let current = self.current_global_names();
        let previous = self
            .last_seen_globals
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        if current == previous {
            return ExternalToolUpdate::default();
        }

        let mut notices = Vec::new();

        for name in current.difference(&previous) {
            notices.push(
                ExternalToolDelta::new(
                    format!("callback:{name}"),
                    ToolConfigChangeOperation::Add,
                    ExternalToolDeltaPhase::Applied,
                )
                .with_tool_count(Some(1)),
            );
        }

        for name in previous.difference(&current) {
            notices.push(ExternalToolDelta::new(
                format!("callback:{name}"),
                ToolConfigChangeOperation::Remove,
                ExternalToolDeltaPhase::Applied,
            ));
        }

        if let Ok(mut last) = self.last_seen_globals.write() {
            *last = current;
        }

        ExternalToolUpdate {
            notices,
            pending: Vec::new(),
        }
    }
}

fn callback_catalog_entry(
    tool: ToolDef,
    execution: meerkat_core::ToolExecutionContract,
) -> ToolCatalogEntry {
    let tool = Arc::new(tool);
    let entry = if let Some(provenance) = tool.provenance.clone() {
        ToolCatalogEntry::session_deferred(tool, true, provenance)
    } else {
        ToolCatalogEntry::session_inline(tool, true)
    };
    entry.with_execution_contract(execution)
}

pub(crate) fn execution_contract_from_wire(
    execution: Option<meerkat_contracts::CallbackToolExecution>,
) -> Result<meerkat_core::ToolExecutionContract, String> {
    let Some(execution) = execution else {
        return Ok(meerkat_core::ToolExecutionContract::default());
    };
    match execution {
        meerkat_contracts::CallbackToolExecution::Fast => {
            Ok(meerkat_core::ToolExecutionContract::default())
        }
        meerkat_contracts::CallbackToolExecution::Detached {
            runner,
            restart_class,
            idempotency_scope,
            submission_timeout_ms,
            credential_scopes,
        } => {
            let runner = meerkat_core::RunnerIdentity::new(runner.name, runner.version)
                .map_err(|error| error.to_string())?;
            let restart_class = match restart_class {
                meerkat_contracts::JobRestartClass::Adoptable => {
                    meerkat_core::RestartClass::Adoptable
                }
                meerkat_contracts::JobRestartClass::CheckpointResumable => {
                    meerkat_core::RestartClass::CheckpointResumable
                }
                meerkat_contracts::JobRestartClass::Replayable => {
                    meerkat_core::RestartClass::Replayable
                }
                meerkat_contracts::JobRestartClass::NonResumable => {
                    meerkat_core::RestartClass::NonResumable
                }
            };
            let idempotency_scope = match idempotency_scope {
                meerkat_contracts::JobIdempotencyScope::ToolCall => {
                    meerkat_core::IdempotencyScope::ToolCall
                }
                meerkat_contracts::JobIdempotencyScope::InteractionAndArguments => {
                    meerkat_core::IdempotencyScope::InteractionAndArguments
                }
                meerkat_contracts::JobIdempotencyScope::HostSemanticKey => {
                    meerkat_core::IdempotencyScope::HostSemanticKey
                }
            };
            let policy = meerkat_core::DetachedToolExecutionPolicy::new(
                runner,
                restart_class,
                idempotency_scope,
                std::time::Duration::from_millis(submission_timeout_ms),
            )
            .map_err(|error| error.to_string())?
            .with_credential_scopes(credential_scopes);
            meerkat_core::ToolExecutionContract::new(
                std::collections::BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
                meerkat_core::ToolExecutionMode::Detached,
                None,
                Some(policy),
            )
            .map_err(|error| error.to_string())
        }
    }
}

fn format_sha256(digest: [u8; 32]) -> String {
    let mut encoded = String::with_capacity("sha256:".len() + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

const CALLBACK_ARGUMENTS_MEDIA_TYPE: &str = "application/vnd.meerkat.callback-arguments+json";

/// The media type of a detached callback job's runner specification since
/// 0.9.3: a [`CallbackSpecification`] record. Jobs submitted before kept only
/// their arguments, under [`CALLBACK_ARGUMENTS_MEDIA_TYPE`].
const CALLBACK_SPECIFICATION_MEDIA_TYPE: &str =
    "application/vnd.meerkat.callback-specification.v2+json";

/// What a detached callback job keeps for its runner: the exact argument
/// text of the call, the declaration of the callback tool that took it
/// ([`meerkat_core::ToolBindingDigest`]), and the route that submitted it
/// ([`CallbackRouteId`]). Any later start, reconcile or cancel through
/// another route (one that took over the mob's callbacks, or registered a
/// tool of the same name and runner, or any route after a restart) compares
/// that declaration with the tool the acting route registers then, so such a
/// route never runs or reaches the job under another declaration.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackSpecification {
    arguments: Box<RawValue>,
    tool_binding_sha256: meerkat_core::ToolBindingDigest,
    submitted_route: String,
}

/// A detached callback job's retained specification, read back.
struct RetainedCallbackSpecification {
    arguments: serde_json::Value,
    /// The declaration it recorded; `None` for one written before 0.9.3.
    tool_binding: Option<meerkat_core::ToolBindingDigest>,
    /// The route that submitted it; `None` for one written before 0.9.3.
    submitted_route: Option<String>,
}

/// The route a start, reconcile or cancel of a detached callback job acts
/// through, and the declaration of the job's tool it registers now (`None`
/// when it registers none).
struct ActingRoute {
    route_id: CallbackRouteId,
    tool_binding: Option<meerkat_core::ToolBindingDigest>,
}

impl RetainedCallbackSpecification {
    /// Whether `acting` may start, reconcile or cancel the job: it is the
    /// route that submitted the job (whatever it registers now), or it
    /// registers exactly the declaration the job recorded.
    fn admits(&self, acting: &ActingRoute) -> bool {
        if self.submitted_route.as_deref() == Some(acting.route_id.as_str()) {
            return true;
        }
        matches!(
            (self.tool_binding.as_ref(), acting.tool_binding.as_ref()),
            (Some(recorded), Some(current)) if recorded == current
        )
    }
}

/// Whether `registry` registers a detached tool of the name and runner the
/// job `spec` describes.
fn registry_owns_detached_callback_spec(
    registry: &CallbackToolRegistry,
    spec: &meerkat::JobSpec,
) -> bool {
    registry
        .state
        .read()
        .map(|state| {
            state.tools.iter().any(|registered| {
                registered.tool.name == spec.tool.name()
                    && registered
                        .execution
                        .detached_policy()
                        .is_some_and(|policy| {
                            policy.runner().name() == spec.runner.name()
                                && policy.runner().version() == spec.runner.version()
                        })
            })
        })
        .unwrap_or(false)
}

/// The declaration of the callback tool `tool_name` that `registry`
/// registers now ([`meerkat_core::ToolBindingDigest`]), `None` when it
/// registers none.
fn registered_tool_binding_in(
    registry: &CallbackToolRegistry,
    tool_name: &str,
) -> Option<meerkat_core::ToolBindingDigest> {
    let state = registry.state.read().ok()?;
    state
        .tools
        .iter()
        .find(|registered| registered.tool.name == tool_name)
        .map(|registered| {
            meerkat_core::ToolBindingDigest::of_catalog_entry(&callback_catalog_entry(
                registered.tool.clone(),
                registered.execution.clone(),
            ))
        })
}

/// The NeedsAttention reason of a detached callback job the acting route may
/// not act on ([`RetainedCallbackSpecification::admits`]).
const CALLBACK_BINDING_CHANGED: &str = "callback_binding_changed";

/// The NeedsAttention reason of a detached callback job whose retained
/// specification no longer decodes.
const CALLBACK_SPECIFICATION_UNREADABLE: &str = "callback_specification_unreadable";

/// Move a detached callback job to NeedsAttention for `reason` instead of
/// acting on it. A job another writer moved first is left as it is.
async fn mark_callback_needs_attention(
    runtime: &DetachedCallbackJobRuntime,
    job_id: &meerkat::JobId,
    reason: &str,
) -> Result<(), String> {
    let reason = meerkat::JobFailureCode::new(reason).map_err(|error| error.to_string())?;
    match runtime
        .service
        .mark_needs_attention(job_id, unix_time_ms()?, reason)
        .await
    {
        Ok(_)
        | Err(meerkat::DetachedJobError::StaleRevision { .. })
        | Err(meerkat::DetachedJobError::InvalidTransition { .. }) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// The job a detached callback call submits, without its runner
/// specification reference, and the call's parsed arguments.
fn detached_callback_job_spec(
    realm_id: &str,
    call: ToolCallView<'_>,
    context: &meerkat_core::ToolDispatchContext,
    plan: &meerkat_core::ResolvedToolExecutionPlan,
) -> Result<(meerkat::JobSpec, serde_json::Value), ToolError> {
    let origin_session_id = context.origin_session_id().cloned().ok_or_else(|| {
        ToolError::execution_failed(
            "detached callback dispatch requires runtime-owned session identity".to_string(),
        )
    })?;
    // A turn has no lineage when its batch merged inputs of several
    // interactions, or when none of its inputs named one.
    let interaction_lineage = context.interaction_lineage_id().ok_or_else(|| {
        ToolError::execution_failed(
            "detached callback dispatch requires runtime-owned interaction lineage: this turn \
             merged several interactions or its input names none"
                .to_string(),
        )
    })?;
    let arguments_sha256 = plan.canonical_arguments_sha256().ok_or_else(|| {
        ToolError::execution_failed(
            "detached callback dispatch requires a root-fenced canonical argument digest"
                .to_string(),
        )
    })?;
    let policy = match plan.kind() {
        meerkat_core::ResolvedExecutionKind::Detached(policy) => policy,
        _ => {
            return Err(ToolError::execution_failed(
                "detached callback owner received a non-detached execution plan".to_string(),
            ));
        }
    };
    let arguments: serde_json::Value = serde_json::from_str(call.args.get())
        .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
    let arguments_hash = format_sha256(arguments_sha256);
    let lineage = interaction_lineage.to_string();
    let submission_key = callback_submission_key(
        realm_id,
        &origin_session_id,
        &lineage,
        call,
        policy,
        &arguments_hash,
        &arguments,
    )?;
    let execution_intent_id = meerkat::ExecutionIntentId::from_string(format!(
        "intent:{lineage}:{}:{arguments_hash}",
        call.name
    ))
    .map_err(|error| ToolError::execution_failed(error.to_string()))?;
    let spec = meerkat::JobSpec::new(
        realm_id,
        origin_session_id,
        execution_intent_id,
        meerkat::InteractionLineageId::from_string(lineage)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        meerkat::ToolIdentity::new(call.name, policy.runner().version())
            .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        meerkat::RunnerIdentity::new(policy.runner().name(), policy.runner().version())
            .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        job_restart_class(policy.restart_class()),
        meerkat::CanonicalArgumentsHash::new(arguments_hash)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?,
        meerkat::JobSubmissionKey::new(submission_key)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?,
    )
    .with_credential_context_refs(match plan.credential_context_refs() {
        meerkat_core::ToolExecutionApplicability::Applicable(references) => references.clone(),
        meerkat_core::ToolExecutionApplicability::NotApplicable => Vec::new(),
    });
    Ok((spec, arguments))
}

/// Resolve a detached callback's submission conflict against the job
/// admitted under the same submission key.
///
/// A job admitted before callback arguments were base64 encoded references
/// the address of the literal arguments, so a replay of the same call now
/// addresses its arguments differently and the store refuses it as another
/// submission. When the admitted job's retained arguments decode to this
/// call's arguments, submit once more under the recorded reference; the
/// arguments hash is the fenced plan's own, and the store's unchanged
/// equivalence check decides every other field. An established mismatch
/// stays a submission conflict; a missing admitted job or a failed read is
/// reported as such, not as a mismatch.
async fn resubmit_under_recorded_arguments(
    runtime: &DetachedCallbackJobRuntime,
    spec: meerkat::JobSpec,
    arguments: &serde_json::Value,
) -> Result<meerkat::JobReceipt, ToolError> {
    let Some(admitted) = runtime
        .store
        .get_by_submission_key(&spec.realm_id, &spec.submission_key)
        .await
        .map_err(|error| ToolError::execution_failed(error.to_string()))?
    else {
        return Err(ToolError::execution_failed(format!(
            "detached callback submission {} conflicted with an admitted job that is unavailable",
            spec.submission_key
        )));
    };
    let Some(recorded_ref) = admitted.spec.runner_specification_ref else {
        return Err(ToolError::execution_failed(
            meerkat::DetachedJobError::SubmissionConflict.to_string(),
        ));
    };
    if read_callback_arguments(runtime, &recorded_ref)
        .await
        .map_err(ToolError::execution_failed)?
        != *arguments
    {
        return Err(ToolError::execution_failed(
            meerkat::DetachedJobError::SubmissionConflict.to_string(),
        ));
    }
    let mut replay = spec;
    replay.runner_specification_ref = Some(recorded_ref);
    // The admitted job's origin member: a replay from a session its member
    // has since left no longer derives it (#1790).
    replay.origin_member_id = admitted.spec.origin_member_id;
    runtime
        .service
        .submit(replay)
        .await
        .map_err(|error| ToolError::execution_failed(error.to_string()))
}

/// Read the retained arguments of a detached callback job.
async fn read_callback_arguments(
    runtime: &DetachedCallbackJobRuntime,
    reference: &meerkat::RunnerSpecificationRef,
) -> Result<serde_json::Value, String> {
    read_callback_specification(runtime, reference)
        .await
        .map(|specification| specification.arguments)
        .map_err(|error| error.to_string())
}

/// Why a detached callback job's retained specification could not be read.
enum SpecificationReadError {
    /// The blob store cannot return it now (an I/O error, a missing blob):
    /// a later pass reads it again.
    Unavailable(String),
    /// It no longer decodes (or the store reports it corrupt): no later pass
    /// reads it either.
    Unreadable(String),
}

impl std::fmt::Display for SpecificationReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(error) => {
                write!(f, "detached callback specification is unavailable: {error}")
            }
            Self::Unreadable(error) => {
                write!(f, "detached callback specification is corrupt: {error}")
            }
        }
    }
}

/// Read the retained specification of a detached callback job, of either
/// version.
async fn read_callback_specification(
    runtime: &DetachedCallbackJobRuntime,
    reference: &meerkat::RunnerSpecificationRef,
) -> Result<RetainedCallbackSpecification, SpecificationReadError> {
    let payload = runtime
        .blob_store
        .get(&meerkat_core::BlobId::new(reference.as_str()))
        .await
        .map_err(|error| match error {
            meerkat_core::BlobStoreError::Corrupt { .. } => {
                SpecificationReadError::Unreadable(error.to_string())
            }
            error => SpecificationReadError::Unavailable(error.to_string()),
        })?;
    decode_callback_specification(&payload).map_err(SpecificationReadError::Unreadable)
}

/// Decode a retained specification: a [`CallbackSpecification`] record
/// under [`CALLBACK_SPECIFICATION_MEDIA_TYPE`] (the base64 of its JSON), or
/// the arguments alone under [`CALLBACK_ARGUMENTS_MEDIA_TYPE`], which
/// recorded no declaration.
fn decode_callback_specification(
    payload: &meerkat_core::BlobPayload,
) -> Result<RetainedCallbackSpecification, String> {
    if payload.media_type != CALLBACK_SPECIFICATION_MEDIA_TYPE {
        return decode_callback_arguments(payload).map(|arguments| RetainedCallbackSpecification {
            arguments,
            tool_binding: None,
            submitted_route: None,
        });
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.data.as_bytes())
        .map_err(|error| format!("invalid base64: {error}"))?;
    let record: CallbackSpecification =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(RetainedCallbackSpecification {
        arguments: serde_json::from_str(record.arguments.get())
            .map_err(|error| error.to_string())?,
        tool_binding: Some(record.tool_binding_sha256),
        submitted_route: Some(record.submitted_route),
    })
}

/// Decode retained callback arguments stored under exactly the callback
/// media type.
///
/// Arguments written before they were base64 encoded hold the literal JSON
/// text, which stores that keep payloads verbatim return unchanged. Callback
/// arguments may be any JSON value, and no base64 of valid JSON starts with
/// a JSON start token, so the first byte after JSON whitespace selects the
/// representation: a start token selects the literal form, anything else the
/// strict base64 of the original text. A malformed selected representation
/// is an error; it is never retried as the other one.
fn decode_callback_arguments(
    payload: &meerkat_core::BlobPayload,
) -> Result<serde_json::Value, String> {
    if payload.media_type != CALLBACK_ARGUMENTS_MEDIA_TYPE {
        return Err(format!(
            "media type {} is not {CALLBACK_ARGUMENTS_MEDIA_TYPE}",
            payload.media_type
        ));
    }
    let first = payload
        .data
        .bytes()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
    if matches!(
        first,
        Some(b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n')
    ) {
        return serde_json::from_str(&payload.data).map_err(|error| error.to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.data.as_bytes())
        .map_err(|error| format!("invalid base64: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn callback_submission_key(
    realm_id: &str,
    origin_session_id: &meerkat_core::SessionId,
    interaction_lineage: &str,
    call: ToolCallView<'_>,
    policy: &meerkat_core::DetachedToolExecutionPolicy,
    arguments_hash: &str,
    arguments: &serde_json::Value,
) -> Result<String, ToolError> {
    let scope = match policy.idempotency_scope() {
        meerkat_core::IdempotencyScope::ToolCall => format!("tool-call:{}", call.id),
        meerkat_core::IdempotencyScope::InteractionAndArguments => {
            format!("interaction:{interaction_lineage}:arguments:{arguments_hash}")
        }
        meerkat_core::IdempotencyScope::HostSemanticKey => {
            let semantic_key = arguments
                .get("idempotency_key")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ToolError::invalid_arguments(
                        call.name,
                        "host_semantic_key execution requires a non-empty string idempotency_key",
                    )
                })?;
            format!("host-semantic:{semantic_key}")
        }
    };
    Ok(format!(
        "callback:{realm_id}:{origin_session_id}:{}:{}:{scope}",
        call.name,
        policy.runner().version(),
    ))
}

fn job_restart_class(class: meerkat_core::RestartClass) -> meerkat::RestartClass {
    match class {
        meerkat_core::RestartClass::Adoptable => meerkat::RestartClass::Adoptable,
        meerkat_core::RestartClass::CheckpointResumable => {
            meerkat::RestartClass::CheckpointResumable
        }
        meerkat_core::RestartClass::Replayable => meerkat::RestartClass::Replayable,
        meerkat_core::RestartClass::NonResumable => meerkat::RestartClass::NonResumable,
    }
}

fn wire_restart_class(class: meerkat::RestartClass) -> meerkat_contracts::JobRestartClass {
    match class {
        meerkat::RestartClass::Adoptable => meerkat_contracts::JobRestartClass::Adoptable,
        meerkat::RestartClass::CheckpointResumable => {
            meerkat_contracts::JobRestartClass::CheckpointResumable
        }
        meerkat::RestartClass::Replayable => meerkat_contracts::JobRestartClass::Replayable,
        meerkat::RestartClass::NonResumable => meerkat_contracts::JobRestartClass::NonResumable,
    }
}

/// Start the next attempt of a runnable detached callback job through the
/// `acting` route. A job that route may not act on
/// ([`RetainedCallbackSpecification::admits`]) is not its to start: it is
/// left as it is for a route that may, or goes to NeedsAttention when its
/// record names no declaration (written before 0.9.3), since no route ever
/// may. A job whose specification no longer decodes goes to NeedsAttention.
async fn start_detached_callback_attempt(
    runtime: DetachedCallbackJobRuntime,
    callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    job_id: meerkat::JobId,
    acting: ActingRoute,
) -> Result<(), String> {
    let stored = runtime
        .store
        .get(&job_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("detached callback job {job_id} disappeared after submission"))?;
    let claimed_at_ms = unix_time_ms()?;
    let runnable = stored.machine_state.lifecycle_phase == meerkat::JobPhase::Queued
        || (stored.machine_state.lifecycle_phase == meerkat::JobPhase::RetryScheduled
            && stored
                .machine_state
                .retry_due_at_ms
                .is_some_and(|due_at_ms| claimed_at_ms >= due_at_ms));
    if !runnable {
        return Ok(());
    }
    let specification_ref = stored
        .spec
        .runner_specification_ref
        .clone()
        .ok_or_else(|| format!("detached callback job {job_id} has no runner specification"))?;
    let specification = match read_callback_specification(&runtime, &specification_ref).await {
        Ok(specification) => specification,
        Err(SpecificationReadError::Unreadable(_)) => {
            return mark_callback_needs_attention(
                &runtime,
                &job_id,
                CALLBACK_SPECIFICATION_UNREADABLE,
            )
            .await;
        }
        Err(error) => return Err(error.to_string()),
    };
    if !specification.admits(&acting) {
        // Another declaration: the job stays queued, unclaimed and
        // unrecorded, for a route serving the declaration it recorded or
        // for the route that submitted it.
        if specification.tool_binding.is_some() {
            return Ok(());
        }
        return mark_callback_needs_attention(&runtime, &job_id, CALLBACK_BINDING_CHANGED).await;
    }

    let lease_expires_at_ms = claimed_at_ms
        .checked_add(u64::try_from(CALLBACK_JOB_INITIAL_LEASE.as_millis()).unwrap_or(u64::MAX))
        .ok_or_else(|| "detached callback lease timestamp overflowed".to_string())?;
    let runner_handle = format!(
        "callback:{job_id}:attempt:{}",
        stored.machine_state.attempt_count.saturating_add(1)
    );
    let claim = match runtime
        .service
        .claim_attempt(
            &job_id,
            meerkat::AttemptClaim::new(
                meerkat::WorkerId::new(format!("rpc-callback:{}", std::process::id()))
                    .map_err(|error| error.to_string())?,
                claimed_at_ms,
                lease_expires_at_ms,
                meerkat::RunnerHandleRef::new(runner_handle.clone())
                    .map_err(|error| error.to_string())?,
            ),
        )
        .await
    {
        Ok(claim) => claim,
        Err(meerkat::DetachedJobError::StaleRevision { .. })
        | Err(meerkat::DetachedJobError::InvalidTransition { .. }) => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let arguments = specification.arguments;
    let credential_scopes = stored
        .spec
        .credential_context_refs
        .iter()
        .flat_map(|reference| match reference {
            meerkat_core::ToolCredentialContextRef::OwningProfile { required_scopes }
            | meerkat_core::ToolCredentialContextRef::AuthBinding {
                required_scopes, ..
            } => required_scopes.iter().cloned().collect::<Vec<_>>(),
        })
        .collect();
    let params = meerkat_contracts::CallbackJobStartParams {
        authority: meerkat_contracts::JobAttemptAuthority {
            job_id: job_id.to_string(),
            attempt_id: claim.attempt_id.to_string(),
            fence: claim.fence.get(),
        },
        runner: meerkat_contracts::JobRunner {
            name: stored.spec.runner.name().to_string(),
            version: stored.spec.runner.version().to_string(),
        },
        restart_class: wire_restart_class(stored.spec.restart_class),
        runner_handle: runner_handle.clone(),
        runner_specification_ref: Some(specification_ref.to_string()),
        arguments,
        credential_scopes,
        resume_checkpoint: claim.resume_checkpoint.as_ref().map(ToString::to_string),
    };
    let result: meerkat_contracts::CallbackJobStartResult =
        match send_callback_request(callback_tx, id_counter, "callback/job/start", &params).await {
            Ok(result) => result,
            Err(error) => {
                // A lost start response is ambiguous: the host may have
                // accepted the committed attempt. Preserve its exact
                // authority for reconciliation instead of terminalizing it.
                return Err(error);
            }
        };
    if !result.accepted || result.runner_handle != runner_handle {
        runtime
            .service
            .mark_needs_attention(
                &job_id,
                unix_time_ms().unwrap_or(claimed_at_ms),
                meerkat::JobFailureCode::new(if result.accepted {
                    "callback_runner_handle_mismatch"
                } else {
                    "callback_start_rejected"
                })
                .map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn send_callback_request<P, R>(
    callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    method: &str,
    params: &P,
) -> Result<R, String>
where
    P: Serialize + ?Sized,
    R: serde::de::DeserializeOwned,
{
    let _invocation_permit = CallbackInvocationAdmission::production()
        .try_acquire()
        .map_err(|error| error.to_string())?;
    let request_id = RpcId::Str(format!(
        "srv-{}",
        id_counter.fetch_add(1, Ordering::Relaxed)
    ));
    let (params_raw, outbound_permit) = RpcOutboundAdmission::production()
        .admit_json(
            params,
            CALLBACK_REQUEST_MAX_PARAMS_BYTES,
            CALLBACK_REQUEST_ENVELOPE_BYTES,
        )
        .map_err(|error| error.to_string())?;
    let request = RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(request_id),
        method: method.to_string(),
        params: Some(params_raw),
    };
    let (response_tx, response_rx) = oneshot::channel();
    callback_tx
        .send(CallbackRequestEnvelope::new(
            request,
            response_tx,
            outbound_permit,
        ))
        .await
        .map_err(|_| "callback channel closed".to_string())?;
    let handoff = tokio::time::timeout(CALLBACK_TIMEOUT, response_rx)
        .await
        .map_err(|_| format!("{method} timed out"))?
        .map_err(|_| format!("{method} response channel dropped"))?;
    if let Some(error) = handoff.response().error.as_ref() {
        return Err(error.message.clone());
    }
    let result = handoff
        .response()
        .result
        .as_deref()
        .ok_or_else(|| format!("{method} response is missing result"))?;
    serde_json::from_str(result.get()).map_err(|error| error.to_string())
}

fn unix_time_ms() -> Result<u64, String> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    u64::try_from(millis).map_err(|_| "wall clock exceeds u64 milliseconds".to_string())
}

fn callback_timeout_error(name: &str) -> ToolError {
    ToolError::timeout(
        name,
        u64::try_from(CALLBACK_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::{ToolProvenance, ToolSourceKind};
    use serde_json::value::RawValue;

    fn make_tool_def(name: &str) -> ToolDef {
        ToolDef {
            audience: Default::default(),
            name: name.into(),
            description: format!("Test tool {name}"),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        }
    }

    fn make_provenanced_tool_def(name: &str) -> ToolDef {
        ToolDef {
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Callback,
                source_id: "callback".into(),
                artifact: None,
            }),
            ..make_tool_def(name)
        }
    }

    fn detached_execution_contract() -> meerkat_core::ToolExecutionContract {
        let policy = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("meta.callback", "v1").unwrap(),
            meerkat_core::RestartClass::CheckpointResumable,
            meerkat_core::IdempotencyScope::InteractionAndArguments,
            std::time::Duration::from_secs(5),
        )
        .unwrap()
        .with_credential_scopes(["device.execute"]);
        meerkat_core::ToolExecutionContract::new(
            std::collections::BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
            meerkat_core::ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn callback_dispatcher_sends_request_awaits_result() {
        let (callback_tx, mut callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

        let handle = tokio::spawn(async move {
            let args = RawValue::from_string("{\"q\":\"test\"}".to_string()).unwrap();
            let call = ToolCallView {
                id: "tc-1",
                name: "search",
                args: &args,
            };
            dispatcher.dispatch(call).await
        });

        let (request, response_tx, _outbound_permit) =
            callback_rx.recv().await.unwrap().into_parts();
        assert_eq!(request.method, "tool/execute");
        assert_eq!(request.id, Some(RpcId::Str("srv-0".to_string())));
        let params: serde_json::Value =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params["tool_use_id"], "tc-1");
        assert_eq!(params["name"], "search");
        assert_eq!(params["arguments"], serde_json::json!({"q": "test"}));

        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        serde_json::json!({"content":"results here","is_error":false}),
                    )
                ))
                .is_ok()
        );

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.result.text_content(), "results here");
        assert!(!result.result.is_error);
    }

    /// Answer one `tool/execute` request with a plain result.
    fn answer_tool_execute(
        request: RpcRequest,
        response_tx: oneshot::Sender<CallbackResponseHandoff>,
    ) {
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        serde_json::json!({"content":"done","is_error":false}),
                    )
                ))
                .is_ok()
        );
    }

    /// A callback tool's `tool/execute` request carries the call's dedupe
    /// key, unchanged, as `params.dedupe_key`. (Unrun until the candidate's
    /// validation.)
    #[tokio::test]
    async fn a_keyed_tool_execute_request_carries_the_dedupe_key() {
        let (callback_tx, mut callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));
        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);
        let args = RawValue::from_string("{\"q\":\"test\"}".to_string()).unwrap();
        let key = meerkat_core::ToolCallDedupeKey::new(
            meerkat_core::RunId::new(),
            3,
            "tc-keyed",
            "search",
            &args,
        )
        .unwrap();
        let expected = serde_json::to_value(&key).unwrap();

        let handle = tokio::spawn(async move {
            let call = ToolCallView {
                id: "tc-keyed",
                name: "search",
                args: &args,
            };
            dispatcher.send_tool_execute(call, Some(key)).await
        });

        let (request, response_tx, _outbound_permit) =
            callback_rx.recv().await.unwrap().into_parts();
        assert_eq!(request.method, "tool/execute");
        let params: serde_json::Value =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params["tool_use_id"], "tc-keyed");
        assert_eq!(params["dedupe_key"], expected);
        assert_eq!(params["dedupe_key"]["call_id"], "tc-keyed");
        assert_eq!(params["dedupe_key"]["round"], 3);
        answer_tool_execute(request, response_tx);

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.result.text_content(), "done");
    }

    /// A call without a dedupe key (outside an agent-loop round) sends a
    /// `tool/execute` request with no `dedupe_key` field, through either
    /// dispatch entry. (Unrun until the candidate's validation.)
    #[tokio::test]
    async fn an_unkeyed_tool_execute_request_omits_the_dedupe_key() {
        for with_context in [false, true] {
            let (callback_tx, mut callback_rx) = mpsc::channel(10);
            let id_counter = Arc::new(AtomicU64::new(0));
            let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));
            let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

            let handle = tokio::spawn(async move {
                let args = RawValue::from_string("{\"q\":\"test\"}".to_string()).unwrap();
                let call = ToolCallView {
                    id: "tc-plain",
                    name: "search",
                    args: &args,
                };
                if with_context {
                    let context = meerkat_core::ToolDispatchContext::default();
                    dispatcher.dispatch_with_context(call, &context).await
                } else {
                    dispatcher.dispatch(call).await
                }
            });

            let (request, response_tx, _outbound_permit) =
                callback_rx.recv().await.unwrap().into_parts();
            let params: serde_json::Value =
                serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
            assert_eq!(params["tool_use_id"], "tc-plain");
            assert!(params.get("dedupe_key").is_none(), "{params}");
            answer_tool_execute(request, response_tx);

            let result = handle.await.unwrap().unwrap();
            assert_eq!(result.result.text_content(), "done");
        }
    }

    async fn dispatch_callback_result(
        payload: serde_json::Value,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("capture")]));
        let dispatcher =
            CallbackToolDispatcher::new(tools, callback_tx, Arc::new(AtomicU64::new(0)), vec![]);
        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-content",
            name: "capture",
            args: &args,
        };
        let respond = async move {
            let (request, response_tx, _outbound_permit) =
                callback_rx.recv().await.unwrap().into_parts();
            assert!(
                response_tx
                    .send(CallbackResponseHandoff::unmetered_for_test(
                        RpcResponse::success(request.id, payload)
                    ))
                    .is_ok()
            );
        };
        let (result, ()) = tokio::join!(dispatcher.dispatch(call), respond);
        result
    }

    #[tokio::test]
    async fn callback_result_preserves_legacy_text_and_error_flag() {
        for is_error in [false, true] {
            let result = dispatch_callback_result(serde_json::json!({
                "content": "legacy result", "is_error": is_error,
            }))
            .await
            .unwrap()
            .result;
            assert_eq!(result.tool_use_id, "tc-content");
            assert_eq!(result.text_content(), "legacy result");
            assert_eq!(result.content.len(), 1);
            assert_eq!(result.is_error, is_error);
        }
    }

    #[tokio::test]
    async fn callback_result_preserves_text_and_inline_image_blocks() {
        let result = dispatch_callback_result(serde_json::json!({
            "content": [
                {"type": "text", "text": "Captured frame"},
                {"type": "image", "media_type": "image/png", "source": "inline", "data": "AQID"},
            ],
            "is_error": false,
        }))
        .await
        .unwrap()
        .result;
        assert_eq!(result.tool_use_id, "tc-content");
        assert_eq!(
            result.content,
            vec![
                meerkat_core::ContentBlock::Text {
                    text: "Captured frame".to_string(),
                },
                meerkat_core::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: meerkat_core::ImageData::Inline {
                        data: "AQID".to_string(),
                    },
                },
            ]
        );
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn callback_result_preserves_error_blocks() {
        let result = dispatch_callback_result(serde_json::json!({
            "content": [{"type": "text", "text": "Tool rejected the action"}],
            "is_error": true,
        }))
        .await
        .unwrap()
        .result;
        assert_eq!(result.tool_use_id, "tc-content");
        assert_eq!(result.text_content(), "Tool rejected the action");
        assert_eq!(result.content.len(), 1);
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn callback_result_rejects_malformed_and_unknown_blocks() {
        for payload in [
            serde_json::json!({"content": [{"type": "image", "source": "inline", "data": "AQID"}]}),
            serde_json::json!({"content": [
                {"type": "text", "text": "Do not return a partial result"},
                {"type": "unknown_future_block"},
            ]}),
        ] {
            let error = dispatch_callback_result(payload).await.unwrap_err();
            assert!(matches!(error, ToolError::ExecutionFailed { .. }));
        }
    }

    #[tokio::test]
    async fn callback_result_without_content_preserves_empty_text_and_error_flag() {
        for is_error in [false, true] {
            let result = dispatch_callback_result(serde_json::json!({"is_error": is_error}))
                .await
                .unwrap()
                .result;
            assert_eq!(result.tool_use_id, "tc-content");
            assert_eq!(
                result.content,
                vec![ContentBlock::Text {
                    text: String::new(),
                }]
            );
            assert_eq!(result.is_error, is_error);
        }
    }

    #[tokio::test]
    async fn callback_dispatcher_timeout_returns_tool_error() {
        let (callback_tx, _callback_rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("slow")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);

        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-2",
            name: "slow",
            args: &args,
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            dispatcher.dispatch(call),
        )
        .await;

        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[test]
    fn callback_catalog_and_resolver_declare_actual_internal_timeout() {
        let (callback_tx, _callback_rx) = mpsc::channel(10);
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("slow")]));
        let dispatcher =
            CallbackToolDispatcher::new(tools, callback_tx, Arc::new(AtomicU64::new(0)), vec![]);
        let catalog = dispatcher.tool_catalog();
        assert_eq!(
            catalog[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Fast
        );
        let args = RawValue::from_string("{}".to_string()).unwrap();
        let deadlines = meerkat_core::ToolDeadlineChain::new(vec![
            meerkat_core::ToolDeadlineContributor::finite(
                meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_secs(600),
            ),
        ])
        .unwrap();
        let plan = dispatcher
            .resolve_execution_plan(
                ToolCallView {
                    id: "tc-resolve",
                    name: "slow",
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default(),
                &meerkat_core::ToolExecutionResolutionContext::new(deadlines),
            )
            .unwrap();

        assert_eq!(plan.deadlines().effective_timeout(), Some(CALLBACK_TIMEOUT));
        assert!(plan.deadlines().contributors().iter().any(|contributor| {
            contributor.owner() == meerkat_core::ToolDeadlineOwner::ToolInternal
                && contributor.timeout() == Some(CALLBACK_TIMEOUT)
        }));
    }

    #[test]
    fn registered_detached_execution_contract_survives_catalog_projection() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let (callback_tx, _callback_rx) = mpsc::channel(1);
        let dispatcher = CallbackToolDispatcher::from_registry(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
        );

        let catalog = dispatcher.tool_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(
            catalog[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Detached
        );
        let policy = catalog[0]
            .execution
            .detached_policy()
            .expect("detached callback policy");
        assert_eq!(policy.runner().name(), "meta.callback");
        assert_eq!(policy.runner().version(), "v1");
        assert_eq!(
            policy.restart_class(),
            meerkat_core::RestartClass::CheckpointResumable
        );
        assert_eq!(
            policy.idempotency_scope(),
            meerkat_core::IdempotencyScope::InteractionAndArguments
        );
        assert_eq!(
            policy.submission_timeout(),
            std::time::Duration::from_secs(5)
        );
        assert!(policy.credential_scopes().contains("device.execute"));
    }

    #[test]
    fn callback_wire_execution_maps_to_detached_core_contract() {
        let contract = execution_contract_from_wire(Some(
            meerkat_contracts::CallbackToolExecution::Detached {
                runner: meerkat_contracts::JobRunner {
                    name: "meta.callback".to_string(),
                    version: "v1".to_string(),
                },
                restart_class: meerkat_contracts::JobRestartClass::Adoptable,
                idempotency_scope: meerkat_contracts::JobIdempotencyScope::InteractionAndArguments,
                submission_timeout_ms: 2_500,
                credential_scopes: vec!["device.execute".to_string()],
            },
        ))
        .unwrap();

        assert_eq!(
            contract.default_mode(),
            meerkat_core::ToolExecutionMode::Detached
        );
        let policy = contract.detached_policy().unwrap();
        assert_eq!(policy.runner().name(), "meta.callback");
        assert_eq!(
            policy.restart_class(),
            meerkat_core::RestartClass::Adoptable
        );
        assert_eq!(
            policy.idempotency_scope(),
            meerkat_core::IdempotencyScope::InteractionAndArguments
        );
        assert_eq!(
            policy.submission_timeout(),
            std::time::Duration::from_millis(2_500)
        );
        assert!(policy.credential_scopes().contains("device.execute"));
    }

    #[test]
    fn callback_wire_execution_rejects_zero_submission_timeout() {
        let error = execution_contract_from_wire(Some(
            meerkat_contracts::CallbackToolExecution::Detached {
                runner: meerkat_contracts::JobRunner {
                    name: "meta.callback".to_string(),
                    version: "v1".to_string(),
                },
                restart_class: meerkat_contracts::JobRestartClass::Replayable,
                idempotency_scope: meerkat_contracts::JobIdempotencyScope::ToolCall,
                submission_timeout_ms: 0,
                credential_scopes: Vec::new(),
            },
        ))
        .unwrap_err();

        assert!(error.contains("greater than zero"));
    }

    #[tokio::test]
    async fn detached_callback_dispatch_commits_receipt_without_waiting_for_host_work() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let (callback_tx, mut callback_rx) = mpsc::channel(2);
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
                registry,
                callback_tx,
                Arc::new(AtomicU64::new(0)),
                vec![],
                "realm-a",
                job_store.clone(),
                blob_store,
            ));
        let args = RawValue::from_string(r#"{"target":"lan"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-detached",
            name: "durable",
            args: &args,
        };
        let context = meerkat_core::ToolDispatchContext::default().with_runtime_identity(
            meerkat_core::SessionId::new(),
            Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(0xfeed_beef),
            )),
        );
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(30),
                ),
            ])
            .unwrap(),
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context,
        )
        .unwrap();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan),
        )
        .await
        .expect("detached submit must not wait for host execution")
        .unwrap();
        let receipt: meerkat_contracts::JobReceipt =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(receipt.status, meerkat_contracts::JobPhase::Queued);
        assert!(!receipt.deduplicated);
        assert!(receipt.awaitable);

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(1), callback_rx.recv())
            .await
            .expect("attempt start should be queued")
            .expect("callback channel");
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/start");
        let start: meerkat_contracts::CallbackJobStartParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(start.authority.job_id, receipt.job_id);
        assert_eq!(start.runner.name, "meta.callback");
        assert_eq!(
            start.restart_class,
            meerkat_contracts::JobRestartClass::CheckpointResumable
        );
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobStartResult {
                            accepted: true,
                            runner_handle: start.runner_handle.clone(),
                        },
                    ),
                ))
                .is_ok()
        );

        let job_id = meerkat::JobId::new(receipt.job_id).unwrap();
        let snapshot = meerkat::DetachedJobStore::get(&*job_store, &job_id)
            .await
            .unwrap()
            .expect("durable job");
        assert!(
            snapshot
                .spec
                .interaction_lineage_id
                .as_str()
                .ends_with("feedbeef")
        );
        assert!(snapshot.spec.runner_specification_ref.is_some());
        assert_eq!(snapshot.machine_state.attempt_count, 1);
    }

    fn standard_base64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn callback_arguments_payload(data: String) -> meerkat_core::BlobPayload {
        meerkat_core::BlobPayload {
            blob_id: meerkat_core::BlobId::new("callback-arguments"),
            media_type: CALLBACK_ARGUMENTS_MEDIA_TYPE.to_string(),
            data,
        }
    }

    #[test]
    fn callback_arguments_decode_from_base64_and_from_legacy_literals_of_every_json_shape() {
        for text in [
            r#"{"target": "lan"}"#,
            r#"[1, "two", {"three": 3}]"#,
            r#""text""#,
            "-1.5",
            "0",
            "1234",
            "true",
            "false",
            "null",
            " \t\r\n{\"padded\": true}",
        ] {
            let expected: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(
                decode_callback_arguments(&callback_arguments_payload(text.to_string())),
                Ok(expected.clone()),
                "legacy literal {text:?}"
            );
            assert_eq!(
                decode_callback_arguments(&callback_arguments_payload(standard_base64(
                    text.as_bytes()
                ))),
                Ok(expected),
                "base64 of {text:?}"
            );
        }
    }

    #[test]
    fn callback_arguments_require_exactly_the_callback_media_type() {
        for media_type in [
            "application/json",
            "application/vnd.meerkat.callback-arguments+json; charset=utf-8",
            "Application/vnd.meerkat.callback-arguments+json",
        ] {
            let mut payload = callback_arguments_payload("null".to_string());
            payload.media_type = media_type.to_string();
            assert!(
                decode_callback_arguments(&payload).is_err(),
                "{media_type} must not decode"
            );
        }
    }

    #[test]
    fn a_malformed_callback_argument_record_fails_in_the_form_its_first_token_selects() {
        // A JSON start token selects the literal form. "nope" is also valid
        // base64, and is still read only as a literal.
        for literal in ["nope", "{\"open\": ", " [1,"] {
            let expected = serde_json::from_str::<serde_json::Value>(literal)
                .expect_err("malformed literal")
                .to_string();
            assert_eq!(
                decode_callback_arguments(&callback_arguments_payload(literal.to_string())),
                Err(expected),
                "{literal:?}"
            );
        }
        // Anything else is the strict base64 of the original text: no
        // whitespace trimming, no missing padding, no byte order mark.
        for encoded in [
            format!(" {}", standard_base64(b"{}")),
            format!("{}\n", standard_base64(b"{}")),
            standard_base64(b"{}").trim_end_matches('=').to_string(),
            format!("\u{feff}{}", standard_base64(b"{}")),
        ] {
            let error = decode_callback_arguments(&callback_arguments_payload(encoded.clone()))
                .expect_err("malformed base64");
            assert!(
                error.starts_with("invalid base64: "),
                "{encoded:?}: {error}"
            );
        }
        // Valid base64 of text that is not JSON is not read as a literal.
        let expected = serde_json::from_slice::<serde_json::Value>(b"not json")
            .expect_err("not JSON")
            .to_string();
        assert_eq!(
            decode_callback_arguments(&callback_arguments_payload(standard_base64(b"not json"))),
            Err(expected)
        );
    }

    /// A verbatim blob store held to the base64 blob contract.
    ///
    /// A payload that is not canonical standard base64 is refused at write,
    /// as a gateway's base64 adapter refuses it, so a write of literal JSON
    /// fails here. Reads return what the inner store holds, so records
    /// planted there keep their stored form.
    struct StrictBase64BlobStore {
        inner: Arc<dyn meerkat_core::BlobStore>,
    }

    #[async_trait]
    impl meerkat_core::BlobStore for StrictBase64BlobStore {
        async fn put_image(
            &self,
            media_type: &str,
            data: &str,
        ) -> Result<meerkat_core::BlobRef, meerkat_core::BlobStoreError> {
            base64::engine::general_purpose::STANDARD
                .decode(data.as_bytes())
                .map_err(|error| {
                    meerkat_core::BlobStoreError::WriteFailed(format!(
                        "invalid blob base64: {error}"
                    ))
                })?;
            self.inner.put_image(media_type, data).await
        }

        async fn get(
            &self,
            blob_id: &meerkat_core::BlobId,
        ) -> Result<meerkat_core::BlobPayload, meerkat_core::BlobStoreError> {
            self.inner.get(blob_id).await
        }

        async fn delete(
            &self,
            blob_id: &meerkat_core::BlobId,
        ) -> Result<(), meerkat_core::BlobStoreError> {
            self.inner.delete(blob_id).await
        }

        fn is_persistent(&self) -> bool {
            self.inner.is_persistent()
        }
    }

    /// Detached-job and blob stores persisted under `root`: the job store,
    /// the verbatim file blob store, and that store held to the base64
    /// contract. Opening them again reopens the same records.
    fn open_detached_callback_stores(
        root: &std::path::Path,
    ) -> (
        Arc<dyn meerkat::DetachedJobStore>,
        Arc<dyn meerkat_core::BlobStore>,
        Arc<dyn meerkat_core::BlobStore>,
    ) {
        let job_store: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::SqliteDetachedJobStore::open(root.join("jobs.db")).unwrap());
        let verbatim: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::FsBlobStore::new(root.join("blobs")));
        let strict: Arc<dyn meerkat_core::BlobStore> = Arc::new(StrictBase64BlobStore {
            inner: Arc::clone(&verbatim),
        });
        (job_store, verbatim, strict)
    }

    /// A dispatcher owning one detached tool, `durable`, whose submissions
    /// are keyed by tool call: the same call id with other arguments is the
    /// same submission.
    fn detached_callback_dispatcher(
        job_store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
    ) -> (
        Arc<dyn AgentToolDispatcher>,
        mpsc::Receiver<CallbackRequestEnvelope>,
    ) {
        let (dispatcher, callback_rx) = detached_callback_dispatcher_registering(
            job_store,
            blob_store,
            make_provenanced_tool_def("durable"),
        );
        (dispatcher, callback_rx)
    }

    /// The detached contract of the fixture's `durable` tool.
    fn durable_detached_contract() -> meerkat_core::ToolExecutionContract {
        let policy = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("meta.callback", "v1").unwrap(),
            meerkat_core::RestartClass::CheckpointResumable,
            meerkat_core::IdempotencyScope::ToolCall,
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        meerkat_core::ToolExecutionContract::new(
            std::collections::BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
            meerkat_core::ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .unwrap()
    }

    /// [`detached_callback_dispatcher`] whose route registers `tool` (under
    /// the fixture's detached contract) as its `durable` tool.
    fn detached_callback_dispatcher_registering(
        job_store: Arc<dyn meerkat::DetachedJobStore>,
        blob_store: Arc<dyn meerkat_core::BlobStore>,
        tool: ToolDef,
    ) -> (
        Arc<CallbackToolDispatcher>,
        mpsc::Receiver<CallbackRequestEnvelope>,
    ) {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(tool, durable_detached_contract())])
            .unwrap();
        let (callback_tx, callback_rx) = mpsc::channel(2);
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            blob_store,
        ));
        (dispatcher, callback_rx)
    }

    /// Wait until the job is in `needs_attention` for a changed callback
    /// binding.
    async fn await_binding_changed(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        job_id: &meerkat::JobId,
    ) {
        await_needs_attention(job_store, job_id, "callback_binding_changed").await;
    }

    /// Wait until the job is in `needs_attention` for `reason`.
    async fn await_needs_attention(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        job_id: &meerkat::JobId,
        reason: &str,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let job = job_store.get(job_id).await.unwrap().expect("job");
                if job.machine_state.lifecycle_phase == meerkat::JobPhase::NeedsAttention {
                    assert_eq!(
                        job.terminal_result,
                        Some(meerkat::JobTerminalResult::NeedsAttention {
                            reason: meerkat::JobFailureCode::new(reason).unwrap(),
                        })
                    );
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the job moves to needs_attention");
    }

    async fn job_phase(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        job_id: &meerkat::JobId,
    ) -> meerkat::JobPhase {
        job_store
            .get(job_id)
            .await
            .unwrap()
            .expect("job")
            .machine_state
            .lifecycle_phase
    }

    fn detached_callback_context() -> meerkat_core::ToolDispatchContext {
        meerkat_core::ToolDispatchContext::default().with_runtime_identity(
            meerkat_core::SessionId::parse("019f74fb-1907-7b21-932d-ab22c4d1f532").unwrap(),
            Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(0x5ca1_ab1e),
            )),
        )
    }

    fn resolve_detached_callback_plan(
        dispatcher: &Arc<dyn AgentToolDispatcher>,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> meerkat_core::ResolvedToolExecutionPlan {
        meerkat_core::resolve_tool_execution_plan_fenced(
            dispatcher,
            call,
            context,
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        std::time::Duration::from_secs(30),
                    ),
                ])
                .unwrap(),
            ),
        )
        .unwrap()
    }

    async fn submit_detached_callback(
        dispatcher: &Arc<dyn AgentToolDispatcher>,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_contracts::JobReceipt, ToolError> {
        let context = detached_callback_context();
        let plan = resolve_detached_callback_plan(dispatcher, call, &context);
        let outcome =
            meerkat_core::dispatch_tool_execution_plan_fenced(dispatcher, call, &context, &plan)
                .await?;
        Ok(serde_json::from_str(&outcome.result.text_content()).unwrap())
    }

    /// Accept the next `callback/job/start` the dispatcher offers and return
    /// its parameters.
    async fn accept_detached_callback_start(
        callback_rx: &mut mpsc::Receiver<CallbackRequestEnvelope>,
    ) -> meerkat_contracts::CallbackJobStartParams {
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), callback_rx.recv())
            .await
            .expect("attempt start should be offered")
            .expect("callback channel");
        let (request, response_tx, _outbound_permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/start");
        let start: meerkat_contracts::CallbackJobStartParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobStartResult {
                            accepted: true,
                            runner_handle: start.runner_handle.clone(),
                        },
                    ),
                ))
                .is_ok()
        );
        start
    }

    #[tokio::test]
    async fn detached_callback_array_arguments_are_stored_as_base64_and_replay_after_reopen() {
        let root = tempfile::tempdir().unwrap();
        let args = RawValue::from_string(r#"[1, "two", {"three": 3}]"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-array",
            name: "durable",
            args: &args,
        };

        let (receipt, reference) = {
            let (job_store, verbatim, strict) = open_detached_callback_stores(root.path());
            let (dispatcher, mut callback_rx) =
                detached_callback_dispatcher(job_store.clone(), strict);
            let receipt = submit_detached_callback(&dispatcher, call).await.unwrap();
            assert!(!receipt.deduplicated);
            let start = accept_detached_callback_start(&mut callback_rx).await;
            assert_eq!(start.arguments, serde_json::json!([1, "two", {"three": 3}]));
            let admitted = job_store
                .get(&meerkat::JobId::new(&receipt.job_id).unwrap())
                .await
                .unwrap()
                .expect("admitted job");
            let reference = admitted
                .spec
                .runner_specification_ref
                .clone()
                .expect("runner specification");
            assert_eq!(
                start.runner_specification_ref.as_deref(),
                Some(reference.as_str())
            );
            assert_eq!(admitted.machine_state.attempt_count, 1);
            // The record (stored as base64) holds exactly the argument bytes
            // the call carried, and the declaration of the tool that took it.
            let stored = verbatim
                .get(&meerkat_core::BlobId::new(reference.as_str()))
                .await
                .unwrap();
            assert_eq!(stored.media_type, CALLBACK_SPECIFICATION_MEDIA_TYPE);
            let record: CallbackSpecification = serde_json::from_slice(
                &base64::engine::general_purpose::STANDARD
                    .decode(stored.data.as_bytes())
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(record.arguments.get(), args.get());
            assert_eq!(
                record.tool_binding_sha256,
                meerkat_core::ToolBindingDigest::of_catalog_entry(&callback_catalog_entry(
                    make_provenanced_tool_def("durable"),
                    durable_detached_contract(),
                ))
            );
            (receipt, reference)
        };

        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let (dispatcher, _callback_rx) = detached_callback_dispatcher(job_store.clone(), strict);
        let replay = submit_detached_callback(&dispatcher, call).await.unwrap();
        assert_eq!(replay.job_id, receipt.job_id);
        assert!(replay.deduplicated);
        let jobs = job_store.list_all(usize::MAX).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].spec.runner_specification_ref.as_ref(),
            Some(&reference)
        );
        assert_eq!(jobs[0].machine_state.attempt_count, 1);
    }

    /// #1790: a job admitted with the delivery address of the mob member
    /// whose session submitted it is the job a replay of the same call joins
    /// after that member moved to a successor session, when the replay no
    /// longer derives the address: the store refuses the replay without it
    /// as another submission, and the conflict path adopts the admitted
    /// job's origin member along with its recorded arguments.
    #[tokio::test]
    async fn a_replay_without_the_origin_member_joins_the_admitted_job() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-member",
            name: "durable",
            args: &args,
        };
        let (dispatcher, _callback_rx) =
            detached_callback_dispatcher(job_store.clone(), strict.clone());

        // The member's session admitted the call with its address.
        let recorded = strict
            .put_artifact(
                CALLBACK_ARGUMENTS_MEDIA_TYPE,
                &base64::engine::general_purpose::STANDARD.encode(args.get().as_bytes()),
            )
            .await
            .unwrap();
        let context = detached_callback_context();
        let plan = resolve_detached_callback_plan(&dispatcher, call, &context);
        let (spec, _) = detached_callback_job_spec("realm-a", call, &context, &plan).unwrap();
        let member = meerkat::OriginMemberId::new("rt:member:team:worker:1").unwrap();
        let admitted = meerkat::DetachedJobService::new(job_store.clone())
            .submit(
                spec.with_runner_specification_ref(
                    meerkat::RunnerSpecificationRef::new(recorded.blob_id.to_string()).unwrap(),
                )
                .with_origin_member_id(member.clone()),
            )
            .await
            .unwrap();

        // The replay derives no member: it joins the admitted job.
        let replay = submit_detached_callback(&dispatcher, call)
            .await
            .expect("the replay joins the admitted job");
        assert_eq!(replay.job_id, admitted.job_id.to_string());
        assert!(replay.deduplicated);
        let jobs = job_store.list_all(usize::MAX).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].spec.origin_member_id, Some(member));
    }

    #[tokio::test]
    async fn legacy_literal_callback_arguments_replay_to_their_admitted_job_after_reopen() {
        for literal in [r#"{"target": "lan"}"#, "null"] {
            let root = tempfile::tempdir().unwrap();
            let args = RawValue::from_string(literal.to_string()).unwrap();
            let call = ToolCallView {
                id: "tc-legacy",
                name: "durable",
                args: &args,
            };

            // Before this fix the dispatcher stored the literal arguments,
            // which a verbatim store kept, and admitted the job under the
            // literal's address.
            let (legacy_job, legacy_reference) = {
                let (job_store, verbatim, strict) = open_detached_callback_stores(root.path());
                let legacy = verbatim
                    .put_artifact(CALLBACK_ARGUMENTS_MEDIA_TYPE, literal)
                    .await
                    .unwrap();
                let legacy_reference =
                    meerkat::RunnerSpecificationRef::new(legacy.blob_id.to_string()).unwrap();
                let (dispatcher, _callback_rx) =
                    detached_callback_dispatcher(job_store.clone(), strict);
                let context = detached_callback_context();
                let plan = resolve_detached_callback_plan(&dispatcher, call, &context);
                let (spec, _) =
                    detached_callback_job_spec("realm-a", call, &context, &plan).unwrap();
                let legacy_job = meerkat::DetachedJobService::new(job_store)
                    .submit(spec.with_runner_specification_ref(legacy_reference.clone()))
                    .await
                    .unwrap();
                (legacy_job, legacy_reference)
            };

            let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
            let (dispatcher, mut callback_rx) =
                detached_callback_dispatcher(job_store.clone(), strict);
            let replay = submit_detached_callback(&dispatcher, call).await.unwrap();
            assert_eq!(replay.job_id, legacy_job.job_id.to_string(), "{literal}");
            assert!(replay.deduplicated, "{literal}");
            // The admitted job was still queued, and its record names no
            // tool declaration (it was written before 0.9.3): its first start
            // moves it to needs_attention instead of offering it.
            await_binding_changed(&job_store, &legacy_job.job_id).await;
            assert!(
                callback_rx.try_recv().is_err(),
                "the legacy job is never offered: {literal}"
            );
            let admitted = job_store
                .get(&legacy_job.job_id)
                .await
                .unwrap()
                .expect("admitted job");
            assert_eq!(admitted.machine_state.attempt_count, 0, "{literal}");

            let again = submit_detached_callback(&dispatcher, call).await.unwrap();
            assert_eq!(again.job_id, replay.job_id, "{literal}");
            assert!(again.deduplicated, "{literal}");

            // The same call id with other arguments is still another
            // submission.
            let changed = RawValue::from_string(r#"{"target": "wan"}"#.to_string()).unwrap();
            let error = submit_detached_callback(
                &dispatcher,
                ToolCallView {
                    id: "tc-legacy",
                    name: "durable",
                    args: &changed,
                },
            )
            .await
            .expect_err("changed arguments conflict");
            assert!(
                error
                    .to_string()
                    .contains(&meerkat::DetachedJobError::SubmissionConflict.to_string()),
                "{literal}: {error}"
            );

            let jobs = job_store.list_all(usize::MAX).await.unwrap();
            assert_eq!(jobs.len(), 1, "{literal}");
            assert_eq!(jobs[0].job_id, legacy_job.job_id, "{literal}");
            assert_eq!(
                jobs[0].spec.runner_specification_ref.as_ref(),
                Some(&legacy_reference),
                "{literal}"
            );
            assert_eq!(jobs[0].revision, admitted.revision, "{literal}");
            assert_eq!(jobs[0].machine_state.attempt_count, 0, "{literal}");
        }
    }

    /// Queue `call` as the route `submitted_route` would have: a v2 record
    /// holding its arguments, the declaration `tool` has under the fixture's
    /// contract and that route, and the job admitted under it, unstarted.
    async fn queue_detached_callback(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        blob_store: &Arc<dyn meerkat_core::BlobStore>,
        call: ToolCallView<'_>,
        tool: ToolDef,
        submitted_route: &CallbackRouteId,
    ) -> meerkat::JobId {
        let declaration = meerkat_core::ToolBindingDigest::of_catalog_entry(
            &callback_catalog_entry(tool.clone(), durable_detached_contract()),
        );
        let record = serde_json::to_vec(&CallbackSpecification {
            arguments: call.args.to_owned(),
            tool_binding_sha256: declaration,
            submitted_route: submitted_route.as_str().to_string(),
        })
        .unwrap();
        queue_detached_callback_under(job_store, blob_store, call, tool, &record).await
    }

    /// Queue `call` as [`queue_detached_callback`] does, under the v2
    /// record `record` exactly as given (corrupt ones included).
    async fn queue_detached_callback_under(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        blob_store: &Arc<dyn meerkat_core::BlobStore>,
        call: ToolCallView<'_>,
        tool: ToolDef,
        record: &[u8],
    ) -> meerkat::JobId {
        let (planner, _callback_rx) =
            detached_callback_dispatcher_registering(job_store.clone(), blob_store.clone(), tool);
        let planner: Arc<dyn AgentToolDispatcher> = planner;
        let context = detached_callback_context();
        let plan = resolve_detached_callback_plan(&planner, call, &context);
        let (spec, _) = detached_callback_job_spec("realm-a", call, &context, &plan).unwrap();
        let stored = blob_store
            .put_artifact(
                CALLBACK_SPECIFICATION_MEDIA_TYPE,
                &base64::engine::general_purpose::STANDARD.encode(record),
            )
            .await
            .unwrap();
        meerkat::DetachedJobService::new(job_store.clone())
            .submit(spec.with_runner_specification_ref(
                meerkat::RunnerSpecificationRef::new(stored.blob_id.to_string()).unwrap(),
            ))
            .await
            .unwrap()
            .job_id
    }

    /// A queued detached callback job is started only by the route that
    /// submitted it or by a route whose tool declares exactly what it
    /// recorded. To any other route it is not its own to start: a connection
    /// registering the same name and runner with another declaration that
    /// reconciles first (as every tools/register does) is offered nothing,
    /// and the job stays queued, unclaimed and unchanged, until its live
    /// submitter starts it. A route declaring what a job recorded (its
    /// creator reconnecting with the same tools) starts it too.
    #[tokio::test]
    async fn a_queued_callback_job_starts_only_under_the_declaration_it_recorded() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let (submitter_route, mut submitter_rx) =
            route_registering(make_provenanced_tool_def("durable"));
        let queued = queue_detached_callback(
            &job_store,
            &strict,
            ToolCallView {
                id: "tc-queued",
                name: "durable",
                args: &args,
            },
            make_provenanced_tool_def("durable"),
            submitter_route.route_id(),
        )
        .await;

        let mut other = make_provenanced_tool_def("durable");
        other.description = "another connection's durable tool".to_string();
        let (claimer, mut claimer_rx) =
            detached_callback_dispatcher_registering(job_store.clone(), strict.clone(), other);
        claimer.reconcile_detached_jobs().await.unwrap();
        assert_eq!(
            offered_method(&mut claimer_rx).await,
            None,
            "the job is offered to a route declaring another tool"
        );
        let job = job_store.get(&queued).await.unwrap().expect("job");
        assert_eq!(
            job.machine_state.lifecycle_phase,
            meerkat::JobPhase::Queued,
            "the job stays queued for a route serving its declaration"
        );
        assert_eq!(job.machine_state.attempt_count, 0);

        let submitter = CallbackToolDispatcher::from_route_with_job_runtime(
            &submitter_route,
            vec![],
            "realm-a",
            job_store.clone(),
            strict.clone(),
        );
        submitter.start_due_detached_jobs().await.unwrap();
        let start = accept_detached_callback_start(&mut submitter_rx).await;
        assert_eq!(start.authority.job_id, queued.to_string());
        assert_eq!(start.arguments, serde_json::json!({"target": "lan"}));

        let kept_job = queue_detached_callback(
            &job_store,
            &strict,
            ToolCallView {
                id: "tc-kept",
                name: "durable",
                args: &args,
            },
            make_provenanced_tool_def("durable"),
            &CallbackRouteId::mint(),
        )
        .await;
        let (creator, mut creator_rx) = detached_callback_dispatcher_registering(
            job_store.clone(),
            strict.clone(),
            make_provenanced_tool_def("durable"),
        );
        creator.start_due_detached_jobs().await.unwrap();
        let start = accept_detached_callback_start(&mut creator_rx).await;
        assert_eq!(start.authority.job_id, kept_job.to_string());
        assert_eq!(start.arguments, serde_json::json!({"target": "lan"}));
    }

    /// Submit a call of the fixture's `durable` tool through a route that
    /// registers it, and abandon the attempt the route is offered: the
    /// route's connection closes with the job running (its claim is
    /// committed before the start is sent).
    async fn submit_and_abandon_detached_callback(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        blob_store: &Arc<dyn meerkat_core::BlobStore>,
        call_id: &str,
        args: &RawValue,
    ) -> meerkat::JobId {
        let (creator, mut creator_rx) = detached_callback_dispatcher_registering(
            job_store.clone(),
            blob_store.clone(),
            make_provenanced_tool_def("durable"),
        );
        let creator: Arc<dyn AgentToolDispatcher> = creator;
        let receipt = submit_detached_callback(
            &creator,
            ToolCallView {
                id: call_id,
                name: "durable",
                args,
            },
        )
        .await
        .unwrap();
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), creator_rx.recv())
            .await
            .expect("the attempt start is offered")
            .expect("callback channel");
        let (request, _response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/start");
        drop(creator_rx);
        let job_id = meerkat::JobId::new(receipt.job_id).unwrap();
        assert_eq!(
            job_store
                .get(&job_id)
                .await
                .unwrap()
                .expect("job")
                .machine_state
                .lifecycle_phase,
            meerkat::JobPhase::Running
        );
        job_id
    }

    /// RED 5 (#222): an active detached callback job is reconciled only
    /// through a route whose tool declares what the job recorded when it was
    /// submitted. Its creator's connection closes with the attempt in
    /// flight. A connection that then serves the tool's name and runner with
    /// another declaration (another description) moves the job to
    /// needs_attention without being offered it; one serving the same
    /// declaration (the creator reconnecting with the same tools) is offered
    /// it, and the job keeps running.
    #[tokio::test]
    async fn an_active_callback_job_is_reconciled_only_under_the_declaration_it_recorded() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();

        let changed_job =
            submit_and_abandon_detached_callback(&job_store, &strict, "tc-changed", &args).await;
        let mut other = make_provenanced_tool_def("durable");
        other.description = "another connection's durable tool".to_string();
        let (claimer, mut claimer_rx) =
            detached_callback_dispatcher_registering(job_store.clone(), strict.clone(), other);
        let reconcile = tokio::spawn(async move { claimer.reconcile_detached_jobs().await });
        // Nothing is offered: no request arrives before the claimer's
        // channel closes or the wait ends.
        let offered =
            match tokio::time::timeout(std::time::Duration::from_millis(500), claimer_rx.recv())
                .await
            {
                Ok(Some(envelope)) => Some(envelope.into_parts().0.method),
                Ok(None) | Err(_) => None,
            };
        assert_eq!(
            offered, None,
            "the job is offered to a route declaring another tool"
        );
        reconcile.await.unwrap().unwrap();
        await_binding_changed(&job_store, &changed_job).await;

        let kept_job =
            submit_and_abandon_detached_callback(&job_store, &strict, "tc-kept", &args).await;
        let (creator, mut creator_rx) = detached_callback_dispatcher_registering(
            job_store.clone(),
            strict.clone(),
            make_provenanced_tool_def("durable"),
        );
        let reconcile = tokio::spawn(async move { creator.reconcile_detached_jobs().await });
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), creator_rx.recv())
            .await
            .expect("the job is offered to its declaration's route")
            .expect("callback channel");
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/reconcile");
        let params: meerkat_contracts::CallbackJobReconcileParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(
            params
                .attempts
                .iter()
                .map(|attempt| attempt.authority.job_id.clone())
                .collect::<Vec<_>>(),
            vec![kept_job.to_string()]
        );
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: params
                                .attempts
                                .iter()
                                .map(|attempt| attempt.authority.clone())
                                .collect(),
                        },
                    ),
                ))
                .is_ok()
        );
        reconcile.await.unwrap().unwrap();
        assert_eq!(
            job_store
                .get(&kept_job)
                .await
                .unwrap()
                .expect("job")
                .machine_state
                .lifecycle_phase,
            meerkat::JobPhase::Running
        );
    }

    /// The route that submitted a detached callback job keeps acting on it
    /// whatever it registers since: its own redefinition of the tool is not
    /// checked. Only another route, and every route after a restart, is held
    /// to the declaration the job recorded.
    #[tokio::test]
    async fn the_submitting_route_reconciles_its_job_after_redefining_its_tool() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                durable_detached_contract(),
            )])
            .unwrap();
        let (callback_tx, mut callback_rx) = mpsc::channel(2);
        let route =
            CallbackRoute::from_parts(callback_tx, Arc::new(AtomicU64::new(0)), registry.clone());
        let submitter: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_route_with_job_runtime(
                &route,
                vec![],
                "realm-a",
                job_store.clone(),
                strict.clone(),
            ));
        let receipt = submit_detached_callback(
            &submitter,
            ToolCallView {
                id: "tc-own",
                name: "durable",
                args: &args,
            },
        )
        .await
        .unwrap();
        // The attempt stays in flight: its start is taken and never answered.
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), callback_rx.recv())
            .await
            .expect("the attempt start is offered")
            .expect("callback channel");
        let (request, _unanswered, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/start");

        // The same connection redefines its tool, then reconciles.
        let mut redefined = make_provenanced_tool_def("durable");
        redefined.description = "the same connection's redefined tool".to_string();
        registry
            .replace_or_add_with_contracts(vec![(redefined, durable_detached_contract())])
            .unwrap();
        let reconciler = Arc::new(CallbackToolDispatcher::from_route_with_job_runtime(
            &route,
            vec![],
            "realm-a",
            job_store.clone(),
            strict.clone(),
        ));
        let reconcile = tokio::spawn(async move { reconciler.reconcile_detached_jobs().await });
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), callback_rx.recv())
            .await
            .expect("the job is offered to the route that submitted it")
            .expect("callback channel");
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/reconcile");
        let params: meerkat_contracts::CallbackJobReconcileParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(
            params
                .attempts
                .iter()
                .map(|attempt| attempt.authority.job_id.clone())
                .collect::<Vec<_>>(),
            vec![receipt.job_id.clone()]
        );
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: params
                                .attempts
                                .iter()
                                .map(|attempt| attempt.authority.clone())
                                .collect(),
                        },
                    ),
                ))
                .is_ok()
        );
        reconcile.await.unwrap().unwrap();
        let job = job_store
            .get(&meerkat::JobId::new(receipt.job_id).unwrap())
            .await
            .unwrap()
            .expect("job");
        assert_eq!(
            job.machine_state.lifecycle_phase,
            meerkat::JobPhase::Running
        );
    }

    /// A registry holding `tool` under the fixture's detached contract, and a
    /// route over it with that route's receiving end.
    fn route_registering(
        tool: ToolDef,
    ) -> (CallbackRoute, mpsc::Receiver<CallbackRequestEnvelope>) {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(tool, durable_detached_contract())])
            .unwrap();
        let (callback_tx, callback_rx) = mpsc::channel(2);
        (
            CallbackRoute::from_parts(callback_tx, Arc::new(AtomicU64::new(0)), registry),
            callback_rx,
        )
    }

    /// A dispatcher over a mob slot `first` holds, whose slot changes hands
    /// between the dispatcher's check of a detached job against the route it
    /// acts through and its send to that route: `first`'s connection closes
    /// and `second` claims the slot. The flag records that it did.
    fn dispatcher_whose_slot_changes_hands(
        job_store: &Arc<dyn meerkat::DetachedJobStore>,
        blob_store: &Arc<dyn meerkat_core::BlobStore>,
        first: (CallbackRoute, mpsc::Receiver<CallbackRequestEnvelope>),
        second: CallbackRoute,
    ) -> (CallbackToolDispatcher, Arc<std::sync::atomic::AtomicBool>) {
        let (first_route, first_rx) = first;
        let slot = Arc::new(MobCallbackRouteSlot::claimed_by(first_route));
        let first_rx = std::sync::Mutex::new(Some(first_rx));
        let handed_over = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handover = {
            let slot = Arc::clone(&slot);
            let handed_over = Arc::clone(&handed_over);
            Arc::new(move || {
                drop(first_rx.lock().unwrap().take());
                slot.claim(&second).unwrap();
                handed_over.store(true, Ordering::SeqCst);
            })
        };
        let dispatcher = CallbackToolDispatcher::from_mob_slot_with_job_runtime(
            slot,
            vec![],
            "realm-a",
            job_store.clone(),
            blob_store.clone(),
        )
        .with_check_send_seam(handover);
        (dispatcher, handed_over)
    }

    /// The method of the next request offered on `callback_rx` within half a
    /// second; `None` when none arrives or the channel closes.
    async fn offered_method(
        callback_rx: &mut mpsc::Receiver<CallbackRequestEnvelope>,
    ) -> Option<String> {
        match tokio::time::timeout(std::time::Duration::from_millis(500), callback_rx.recv()).await
        {
            Ok(Some(envelope)) => Some(envelope.into_parts().0.method),
            Ok(None) | Err(_) => None,
        }
    }

    /// A reconcile sends to the route it checked. The mob slot's holder
    /// declares what the active job recorded and is checked; before the send,
    /// its connection closes and a connection declaring another tool claims
    /// the slot. That new holder, never checked, is offered nothing, and the
    /// job keeps running.
    #[tokio::test]
    async fn a_reconcile_sends_only_to_the_route_it_checked() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let job =
            submit_and_abandon_detached_callback(&job_store, &strict, "tc-handover", &args).await;
        let mut other = make_provenanced_tool_def("durable");
        other.description = "another connection's durable tool".to_string();
        let (second, mut second_rx) = route_registering(other);
        let (dispatcher, handed_over) = dispatcher_whose_slot_changes_hands(
            &job_store,
            &strict,
            route_registering(make_provenanced_tool_def("durable")),
            second,
        );
        let reconcile = tokio::spawn(async move { dispatcher.reconcile_detached_jobs().await });
        assert_eq!(
            offered_method(&mut second_rx).await,
            None,
            "the job is offered to a route that was never checked"
        );
        assert!(
            handed_over.load(Ordering::SeqCst),
            "the slot changed hands between the check and the send"
        );
        assert!(
            reconcile.await.unwrap().is_err(),
            "the checked route closed before the send"
        );
        assert_eq!(
            job_phase(&job_store, &job).await,
            meerkat::JobPhase::Running
        );
    }

    /// A cancel sends to the route it checked, as a reconcile does.
    #[tokio::test]
    async fn a_cancel_sends_only_to_the_route_it_checked() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let job =
            submit_and_abandon_detached_callback(&job_store, &strict, "tc-handover", &args).await;
        meerkat::DetachedJobService::new(job_store.clone())
            .request_cancel(&job)
            .await
            .unwrap();
        let mut other = make_provenanced_tool_def("durable");
        other.description = "another connection's durable tool".to_string();
        let (second, mut second_rx) = route_registering(other);
        let (dispatcher, handed_over) = dispatcher_whose_slot_changes_hands(
            &job_store,
            &strict,
            route_registering(make_provenanced_tool_def("durable")),
            second,
        );
        let cancelled = job.clone();
        let cancel = tokio::spawn(async move { dispatcher.cancel_detached_job(&cancelled).await });
        assert_eq!(
            offered_method(&mut second_rx).await,
            None,
            "the cancel goes to a route that was never checked"
        );
        assert!(
            handed_over.load(Ordering::SeqCst),
            "the slot changed hands between the check and the send"
        );
        assert!(
            cancel.await.unwrap().is_err(),
            "the checked route closed before the send"
        );
        assert_eq!(
            job_phase(&job_store, &job).await,
            meerkat::JobPhase::Running
        );
    }

    /// A job whose retained specification no longer decodes goes to
    /// needs_attention (`callback_specification_unreadable`) at its next
    /// start or reconcile, instead of being skipped on every pass; nothing
    /// is offered for it.
    #[tokio::test]
    async fn an_unreadable_specification_moves_the_job_to_needs_attention() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let truncated = br#"{"arguments": {"target": "lan"}, "tool_binding_sha256": "#;
        let queued = queue_detached_callback_under(
            &job_store,
            &strict,
            ToolCallView {
                id: "tc-queued",
                name: "durable",
                args: &args,
            },
            make_provenanced_tool_def("durable"),
            truncated,
        )
        .await;
        let active = queue_detached_callback_under(
            &job_store,
            &strict,
            ToolCallView {
                id: "tc-active",
                name: "durable",
                args: &args,
            },
            make_provenanced_tool_def("durable"),
            truncated,
        )
        .await;
        let claimed_at_ms = unix_time_ms().unwrap();
        meerkat::DetachedJobService::new(job_store.clone())
            .claim_attempt(
                &active,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-before").unwrap(),
                    claimed_at_ms,
                    claimed_at_ms + 120_000,
                    meerkat::RunnerHandleRef::new("handle-before").unwrap(),
                ),
            )
            .await
            .unwrap();

        let (dispatcher, mut callback_rx) = detached_callback_dispatcher_registering(
            job_store.clone(),
            strict.clone(),
            make_provenanced_tool_def("durable"),
        );
        dispatcher.reconcile_detached_jobs().await.unwrap();
        await_needs_attention(&job_store, &active, "callback_specification_unreadable").await;
        await_needs_attention(&job_store, &queued, "callback_specification_unreadable").await;
        assert!(
            callback_rx.try_recv().is_err(),
            "nothing is offered for an unreadable job"
        );
    }

    /// A blob store whose reads fail while `failing` is set, as a store that
    /// cannot be reached would.
    struct FlakyBlobStore {
        inner: Arc<dyn meerkat_core::BlobStore>,
        failing: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl meerkat_core::BlobStore for FlakyBlobStore {
        async fn put_image(
            &self,
            media_type: &str,
            data: &str,
        ) -> Result<meerkat_core::BlobRef, meerkat_core::BlobStoreError> {
            self.inner.put_image(media_type, data).await
        }

        async fn get(
            &self,
            blob_id: &meerkat_core::BlobId,
        ) -> Result<meerkat_core::BlobPayload, meerkat_core::BlobStoreError> {
            if self.failing.load(Ordering::SeqCst) {
                return Err(meerkat_core::BlobStoreError::ReadFailed(
                    "the blob store is unreachable".to_string(),
                ));
            }
            self.inner.get(blob_id).await
        }

        async fn delete(
            &self,
            blob_id: &meerkat_core::BlobId,
        ) -> Result<(), meerkat_core::BlobStoreError> {
            self.inner.delete(blob_id).await
        }

        fn is_persistent(&self) -> bool {
            self.inner.is_persistent()
        }
    }

    /// A job whose specification the blob store cannot return now is left as
    /// it is (with a warning), and a later pass reconciles it once the store
    /// answers again.
    #[tokio::test]
    async fn an_unavailable_specification_leaves_the_job_for_a_later_pass() {
        let root = tempfile::tempdir().unwrap();
        let (job_store, _verbatim, strict) = open_detached_callback_stores(root.path());
        let flaky = Arc::new(FlakyBlobStore {
            inner: strict,
            failing: std::sync::atomic::AtomicBool::new(false),
        });
        let blobs: Arc<dyn meerkat_core::BlobStore> = flaky.clone();
        let args = RawValue::from_string(r#"{"target": "lan"}"#.to_string()).unwrap();
        let job = submit_and_abandon_detached_callback(&job_store, &blobs, "tc-flaky", &args).await;

        flaky.failing.store(true, Ordering::SeqCst);
        let (first, mut first_rx) = detached_callback_dispatcher_registering(
            job_store.clone(),
            blobs.clone(),
            make_provenanced_tool_def("durable"),
        );
        first.reconcile_detached_jobs().await.unwrap();
        assert!(
            first_rx.try_recv().is_err(),
            "nothing is offered while the specification is unavailable"
        );
        assert_eq!(
            job_phase(&job_store, &job).await,
            meerkat::JobPhase::Running
        );

        flaky.failing.store(false, Ordering::SeqCst);
        let (second, mut second_rx) = detached_callback_dispatcher_registering(
            job_store.clone(),
            blobs.clone(),
            make_provenanced_tool_def("durable"),
        );
        let reconcile = tokio::spawn(async move { second.reconcile_detached_jobs().await });
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(5), second_rx.recv())
            .await
            .expect("the job is reconciled once its specification reads again")
            .expect("callback channel");
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/reconcile");
        let params: meerkat_contracts::CallbackJobReconcileParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: params
                                .attempts
                                .iter()
                                .map(|attempt| attempt.authority.clone())
                                .collect(),
                        },
                    ),
                ))
                .is_ok()
        );
        reconcile.await.unwrap().unwrap();
        assert_eq!(
            job_phase(&job_store, &job).await,
            meerkat::JobPhase::Running
        );
    }

    #[tokio::test]
    async fn lost_start_response_preserves_claim_for_reconciliation() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let (callback_tx, callback_rx) = mpsc::channel(1);
        drop(callback_rx);
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
                registry,
                callback_tx,
                Arc::new(AtomicU64::new(0)),
                vec![],
                "realm-a",
                job_store.clone(),
                blob_store,
            ));
        let args = RawValue::from_string(r#"{"target":"lan"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-lost-start",
            name: "durable",
            args: &args,
        };
        let context = meerkat_core::ToolDispatchContext::default().with_runtime_identity(
            meerkat_core::SessionId::new(),
            Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(0xfeed_f00d),
            )),
        );
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(30),
                ),
            ])
            .unwrap(),
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context,
        )
        .unwrap();
        let outcome =
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
                .await
                .unwrap();
        let receipt: meerkat_contracts::JobReceipt =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let job_id = meerkat::JobId::new(receipt.job_id).unwrap();
        let stored = meerkat::DetachedJobStore::get(&*job_store, &job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.machine_state.lifecycle_phase,
            meerkat::JobPhase::Running
        );
        assert_eq!(stored.machine_state.attempt_count, 1);
        assert_eq!(stored.machine_state.current_fence, 1);
        assert!(stored.machine_state.terminal_kind.is_none());
    }

    /// Retain the specification a submission of `arguments` through a route
    /// registering the `durable` tool under [`detached_execution_contract`]
    /// keeps: the record of its arguments and that tool's declaration.
    async fn retain_durable_callback_specification(
        blob_store: &dyn meerkat_core::BlobStore,
        arguments: &str,
    ) -> meerkat::RunnerSpecificationRef {
        let record = serde_json::to_vec(&CallbackSpecification {
            arguments: RawValue::from_string(arguments.to_string()).unwrap(),
            tool_binding_sha256: meerkat_core::ToolBindingDigest::of_catalog_entry(
                &callback_catalog_entry(
                    make_provenanced_tool_def("durable"),
                    detached_execution_contract(),
                ),
            ),
            // Another route submitted it.
            submitted_route: CallbackRouteId::mint().as_str().to_string(),
        })
        .unwrap();
        let stored = blob_store
            .put_artifact(
                CALLBACK_SPECIFICATION_MEDIA_TYPE,
                &base64::engine::general_purpose::STANDARD.encode(record),
            )
            .await
            .unwrap();
        meerkat::RunnerSpecificationRef::new(stored.blob_id.to_string()).unwrap()
    }

    #[tokio::test]
    async fn reconcile_reuses_committed_attempt_fence_lease_checkpoint_and_handle() {
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let service = meerkat::DetachedJobService::new(job_store.clone());
        let session_id = meerkat_core::SessionId::new();
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let specification =
            retain_durable_callback_specification(&*blob_store, r#"{"target":"lan"}"#).await;
        let receipt = service
            .submit(
                meerkat::JobSpec::new(
                    "realm-a",
                    session_id,
                    meerkat::ExecutionIntentId::from_string("intent-reconcile").unwrap(),
                    meerkat::InteractionLineageId::from_string("lineage-reconcile").unwrap(),
                    meerkat::ToolIdentity::new("durable", "v1").unwrap(),
                    meerkat::RunnerIdentity::new("meta.callback", "v1").unwrap(),
                    meerkat::RestartClass::Adoptable,
                    meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                    meerkat::JobSubmissionKey::new("reconcile-same-authority").unwrap(),
                )
                .with_runner_specification_ref(specification),
            )
            .await
            .unwrap();
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-original").unwrap(),
                    10,
                    u64::MAX - 1,
                    meerkat::RunnerHandleRef::new("host-handle-original").unwrap(),
                ),
            )
            .await
            .unwrap();
        service
            .record_checkpoint(
                &receipt.job_id,
                meerkat::AttemptWriteAuthority::from(&claim),
                meerkat::CheckpointRef::new("checkpoint-original").unwrap(),
                11,
            )
            .await
            .unwrap();
        let before = service.get(&receipt.job_id).await.unwrap().unwrap();

        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            blob_store,
        ));
        let reconcile = tokio::spawn({
            let dispatcher = Arc::clone(&dispatcher);
            async move { dispatcher.reconcile_detached_jobs().await }
        });
        let envelope = callback_rx.recv().await.unwrap();
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/reconcile");
        let params: meerkat_contracts::CallbackJobReconcileParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params.attempts.len(), 1);
        let attempt = &params.attempts[0];
        assert_eq!(attempt.authority.attempt_id, claim.attempt_id.to_string());
        assert_eq!(attempt.authority.fence, claim.fence.get());
        assert_eq!(attempt.runner_handle, "host-handle-original");
        assert_eq!(
            attempt.checkpoint_ref.as_deref(),
            Some("checkpoint-original")
        );
        assert_eq!(attempt.lease_expires_at_ms, u64::MAX - 1);
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: vec![attempt.authority.clone()],
                        },
                    )
                ))
                .is_ok()
        );
        reconcile.await.unwrap().unwrap();

        let after = service.get(&receipt.job_id).await.unwrap().unwrap();
        assert_eq!(after.attempt_count, before.attempt_count);
        assert_eq!(after.current_attempt_id, before.current_attempt_id);
        assert_eq!(after.current_fence, before.current_fence);
        assert_eq!(after.lease_expires_at_ms, before.lease_expires_at_ms);
        assert_eq!(after.checkpoint_ref, before.checkpoint_ref);
        assert_eq!(after.runner_handle, before.runner_handle);
    }

    #[tokio::test]
    async fn cancel_uses_committed_attempt_authority_without_advancing_fence() {
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let service = meerkat::DetachedJobService::new(job_store.clone());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let specification =
            retain_durable_callback_specification(&*blob_store, r#"{"target":"lan"}"#).await;
        let receipt = service
            .submit(
                meerkat::JobSpec::new(
                    "realm-a",
                    meerkat_core::SessionId::new(),
                    meerkat::ExecutionIntentId::from_string("intent-cancel").unwrap(),
                    meerkat::InteractionLineageId::from_string("lineage-cancel").unwrap(),
                    meerkat::ToolIdentity::new("durable", "v1").unwrap(),
                    meerkat::RunnerIdentity::new("meta.callback", "v1").unwrap(),
                    meerkat::RestartClass::Adoptable,
                    meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                    meerkat::JobSubmissionKey::new("cancel-same-authority").unwrap(),
                )
                .with_runner_specification_ref(specification),
            )
            .await
            .unwrap();
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-original").unwrap(),
                    10,
                    10_000,
                    meerkat::RunnerHandleRef::new("host-handle-original").unwrap(),
                ),
            )
            .await
            .unwrap();
        service.request_cancel(&receipt.job_id).await.unwrap();
        let before = service.get(&receipt.job_id).await.unwrap().unwrap();

        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            blob_store,
        ));
        let cancelling = tokio::spawn({
            let dispatcher = Arc::clone(&dispatcher);
            let job_id = receipt.job_id.clone();
            async move { dispatcher.cancel_detached_job(&job_id).await }
        });
        let envelope = callback_rx.recv().await.unwrap();
        let (request, response_tx, _permit) = envelope.into_parts();
        assert_eq!(request.method, "callback/job/cancel");
        let params: meerkat_contracts::CallbackJobCancelParams =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert_eq!(params.authority.job_id, receipt.job_id.to_string());
        assert_eq!(params.authority.attempt_id, claim.attempt_id.to_string());
        assert_eq!(params.authority.fence, claim.fence.get());
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobCancelResult { accepted: true },
                    ),
                ))
                .is_ok()
        );
        cancelling.await.unwrap().unwrap();

        let after = service.get(&receipt.job_id).await.unwrap().unwrap();
        assert_eq!(after.attempt_count, before.attempt_count);
        assert_eq!(after.current_attempt_id, before.current_attempt_id);
        assert_eq!(after.current_fence, before.current_fence);
        assert_eq!(after.lease_expires_at_ms, before.lease_expires_at_ms);
    }

    #[tokio::test]
    async fn expired_omitted_non_resumable_attempt_becomes_worker_lost_without_new_fence() {
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let service = meerkat::DetachedJobService::new(job_store.clone());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let specification =
            retain_durable_callback_specification(&*blob_store, r#"{"target":"lan"}"#).await;
        let receipt = service
            .submit(
                meerkat::JobSpec::new(
                    "realm-a",
                    meerkat_core::SessionId::new(),
                    meerkat::ExecutionIntentId::from_string("intent-lost").unwrap(),
                    meerkat::InteractionLineageId::from_string("lineage-lost").unwrap(),
                    meerkat::ToolIdentity::new("durable", "v1").unwrap(),
                    meerkat::RunnerIdentity::new("meta.callback", "v1").unwrap(),
                    meerkat::RestartClass::NonResumable,
                    meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                    meerkat::JobSubmissionKey::new("lost-no-replay").unwrap(),
                )
                .with_runner_specification_ref(specification),
            )
            .await
            .unwrap();
        let claim = service
            .claim_attempt(
                &receipt.job_id,
                meerkat::AttemptClaim::new(
                    meerkat::WorkerId::new("worker-original").unwrap(),
                    1,
                    2,
                    meerkat::RunnerHandleRef::new("host-handle-original").unwrap(),
                ),
            )
            .await
            .unwrap();
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let dispatcher = Arc::new(CallbackToolDispatcher::from_registry_with_job_runtime(
            registry,
            callback_tx,
            Arc::new(AtomicU64::new(0)),
            vec![],
            "realm-a",
            job_store,
            blob_store,
        ));
        let reconciling = tokio::spawn({
            let dispatcher = Arc::clone(&dispatcher);
            async move { dispatcher.reconcile_detached_jobs().await }
        });
        let envelope = callback_rx.recv().await.unwrap();
        let (request, response_tx, _permit) = envelope.into_parts();
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        meerkat_contracts::CallbackJobReconcileResult {
                            live_attempts: Vec::new(),
                        },
                    ),
                ))
                .is_ok()
        );
        reconciling.await.unwrap().unwrap();

        let after = service.get(&receipt.job_id).await.unwrap().unwrap();
        assert_eq!(after.phase, meerkat::JobPhase::WorkerLost);
        assert_eq!(after.attempt_count, claim.attempt_count);
        assert_eq!(after.current_attempt_id, Some(claim.attempt_id));
        assert_eq!(after.current_fence, claim.fence);
    }

    #[test]
    fn callback_internal_expiry_is_typed_timeout() {
        assert!(matches!(
            callback_timeout_error("slow"),
            ToolError::Timeout {
                name,
                timeout_ms: 120_000,
            } if name == "slow"
        ));
    }

    #[tokio::test]
    async fn callback_identical_metadata_reregistration_requires_fresh_resolution() {
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add(vec![make_tool_def("replaceable")])
            .unwrap();
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_registry(
                Arc::clone(&registry),
                callback_tx,
                Arc::new(AtomicU64::new(0)),
                vec![],
            ));
        let args = RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-reregister",
            name: "replaceable",
            args: &args,
        };
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(600),
                ),
            ])
            .unwrap(),
        );
        let dispatch_context = meerkat_core::ToolDispatchContext::default();
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &dispatch_context,
            &resolution_context,
        )
        .unwrap();

        registry
            .replace_or_add(vec![make_tool_def("replaceable")])
            .unwrap();

        let error = meerkat_core::dispatch_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &dispatch_context,
            &plan,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            }
        ));
        assert!(
            callback_rx.try_recv().is_err(),
            "a stale plan must be rejected before callback delivery"
        );
    }

    #[test]
    fn callback_invocation_admission_caps_pending_work_process_wide() {
        let admission = CallbackInvocationAdmission::new(1);
        let held = admission.try_acquire().expect("first callback is admitted");
        assert!(
            admission.try_acquire().is_err(),
            "a second pending callback must fail closed"
        );
        drop(held);
        assert!(admission.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn oversized_callback_arguments_never_enter_the_request_queue() {
        let (callback_tx, mut callback_rx) = mpsc::channel(1);
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("search")]));
        let dispatcher =
            CallbackToolDispatcher::new(tools, callback_tx, Arc::new(AtomicU64::new(0)), vec![]);
        let args = RawValue::from_string(format!(
            "\"{}\"",
            "x".repeat(CALLBACK_REQUEST_MAX_PARAMS_BYTES)
        ))
        .unwrap();
        let result = dispatcher
            .dispatch(ToolCallView {
                id: "tc-oversized",
                name: "search",
                args: &args,
            })
            .await;

        assert!(result.is_err());
        assert!(matches!(
            callback_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn poll_external_updates_detects_additions_and_removals() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        registry.replace_or_add(vec![make_tool_def("a")]).unwrap();
        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            vec![],
        );

        // No change yet.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());

        // Add a tool.
        registry.replace_or_add(vec![make_tool_def("b")]).unwrap();
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:b");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);

        // Remove original tool.
        registry
            .state
            .write()
            .unwrap()
            .tools
            .retain(|registered| registered.tool.name != "a");
        registry.generation.fetch_add(1, Ordering::AcqRel);
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:a");
        assert_eq!(
            update.notices[0].operation,
            ToolConfigChangeOperation::Remove
        );

        // No change.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());
    }

    #[tokio::test]
    async fn tools_reflects_live_registered_list() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        registry.replace_or_add(vec![make_tool_def("x")]).unwrap();
        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            vec![],
        );
        assert_eq!(dispatcher.tools().len(), 1);

        registry.replace_or_add(vec![make_tool_def("y")]).unwrap();
        assert_eq!(dispatcher.tools().len(), 2);

        registry.state.write().unwrap().tools.clear();
        registry.generation.fetch_add(1, Ordering::AcqRel);
        assert_eq!(dispatcher.tools().len(), 0);
    }

    #[tokio::test]
    async fn tool_catalog_reflects_late_registered_provenanced_tools() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add(vec![make_provenanced_tool_def("secret_lookup")])
            .unwrap();
        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            vec![],
        );
        assert_eq!(
            dispatcher
                .tool_catalog()
                .iter()
                .map(|entry| entry.tool.name.clone())
                .collect::<Vec<_>>(),
            vec!["secret_lookup".to_string()]
        );

        registry
            .replace_or_add(vec![make_provenanced_tool_def("secret_audit")])
            .unwrap();

        let names = dispatcher
            .tool_catalog()
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect::<Vec<_>>();
        assert!(names.contains(&"secret_lookup".to_string()));
        assert!(names.contains(&"secret_audit".to_string()));
    }

    #[tokio::test]
    async fn inline_tools_take_precedence_over_globals() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        // Global has "shared" and "global_only"
        let tools = Arc::new(StdRwLock::new(vec![
            make_tool_def("shared"),
            make_tool_def("global_only"),
        ]));
        // Inline has "shared" and "inline_only"
        let inline = vec![make_tool_def("shared"), make_tool_def("inline_only")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        let names: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        // Should have: shared (from inline), inline_only, global_only
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"shared".to_string()));
        assert!(names.contains(&"inline_only".to_string()));
        assert!(names.contains(&"global_only".to_string()));

        // "shared" appears exactly once (inline wins, global filtered)
        assert_eq!(names.iter().filter(|n| *n == "shared").count(), 1);
    }

    #[tokio::test]
    async fn tool_catalog_prefers_inline_winners_and_reports_exact_support() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![
            make_tool_def("shared"),
            make_provenanced_tool_def("global_only"),
        ]));
        let inline = vec![make_tool_def("shared"), make_tool_def("inline_only")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        assert!(
            dispatcher.tool_catalog_capabilities().exact_catalog,
            "callback dispatcher should expose an exact catalog"
        );

        let catalog = dispatcher.tool_catalog();
        let names: Vec<_> = catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
            .collect();
        assert_eq!(names.len(), 3);
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "shared")
                .count(),
            1
        );
        assert!(names.contains(&"inline_only".to_string()));
        assert!(names.contains(&"global_only".to_string()));
        assert!(matches!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "shared")
                .unwrap()
                .deferred_eligibility,
            meerkat_core::ToolCatalogDeferredEligibility::InlineOnly
        ));
        assert!(matches!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "global_only")
                .unwrap()
                .deferred_eligibility,
            meerkat_core::ToolCatalogDeferredEligibility::DeferredEligible { .. }
        ));
    }

    #[tokio::test]
    async fn unprovenanced_registered_tools_stay_inline_only_in_the_catalog() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("global_only")]));

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, vec![]);
        let catalog = dispatcher.tool_catalog();

        assert!(matches!(
            catalog
                .iter()
                .find(|entry| entry.tool.name == "global_only")
                .unwrap()
                .deferred_eligibility,
            meerkat_core::ToolCatalogDeferredEligibility::InlineOnly
        ));
    }

    #[tokio::test]
    async fn inline_tools_do_not_produce_poll_deltas() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![]));
        let inline = vec![make_tool_def("static_tool")];

        let dispatcher = CallbackToolDispatcher::new(tools, callback_tx, id_counter, inline);

        // Inline tool exists but should not produce a delta.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());
    }

    #[tokio::test]
    async fn dynamic_updates_work_with_inline_tools_present() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let registry = Arc::new(CallbackToolRegistry::default());
        let inline = vec![make_tool_def("my_inline")];

        let dispatcher = CallbackToolDispatcher::from_registry(
            Arc::clone(&registry),
            callback_tx,
            id_counter,
            inline,
        );
        assert_eq!(dispatcher.tools().len(), 1); // just inline

        // Register a global tool later.
        registry
            .replace_or_add(vec![make_tool_def("late_global")])
            .unwrap();
        assert_eq!(dispatcher.tools().len(), 2); // inline + late_global

        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:late_global");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);
    }

    fn is_unclaimed_route_error(result: &Result<ToolDispatchOutcome, ToolError>) -> bool {
        matches!(
            result,
            Err(ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::CallbackRouteUnclaimed,
                ..
            })
        )
    }

    /// Answer the next `tool/execute` request on `rx` with `content` and
    /// return the request's params.
    async fn answer_next_callback(
        rx: &mut mpsc::Receiver<CallbackRequestEnvelope>,
        content: &str,
    ) -> serde_json::Value {
        let (request, response_tx, _outbound_permit) = rx.recv().await.unwrap().into_parts();
        assert_eq!(request.method, "tool/execute");
        let params: serde_json::Value =
            serde_json::from_str(request.params.as_deref().unwrap().get()).unwrap();
        assert!(
            response_tx
                .send(CallbackResponseHandoff::unmetered_for_test(
                    RpcResponse::success(
                        request.id,
                        serde_json::json!({"content": content, "is_error": false}),
                    )
                ))
                .is_ok()
        );
        params
    }

    fn route_with_tool(name: &str) -> (CallbackRoute, mpsc::Receiver<CallbackRequestEnvelope>) {
        let (tx, rx) = mpsc::channel(4);
        let route = CallbackRoute::new(tx);
        route
            .registry()
            .replace_or_add(vec![make_tool_def(name)])
            .unwrap();
        (route, rx)
    }

    #[tokio::test]
    async fn mob_slot_dispatcher_follows_a_route_claimed_after_it_was_built() {
        let slot = Arc::new(MobCallbackRouteSlot::unclaimed());
        let dispatcher = CallbackToolDispatcher::from_mob_slot(Arc::clone(&slot), vec![]);
        let args = RawValue::from_string("{}".to_string()).unwrap();

        // Before any claim, a member's call fails typed, not as an unknown
        // tool, and the dispatcher advertises nothing.
        assert!(dispatcher.tools().is_empty());
        let unclaimed = dispatcher
            .dispatch(ToolCallView {
                id: "tc-unclaimed",
                name: "search",
                args: &args,
            })
            .await;
        assert!(is_unclaimed_route_error(&unclaimed), "{unclaimed:?}");

        // A claim reaches the dispatcher built before it.
        let (route, mut rx) = route_with_tool("search");
        assert_eq!(slot.claim(&route), Ok(()));
        let names: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(names, vec!["search".to_string()]);
        let call = dispatcher.dispatch(ToolCallView {
            id: "tc-claimed",
            name: "search",
            args: &args,
        });
        let (outcome, params) = tokio::join!(call, answer_next_callback(&mut rx, "found"));
        assert_eq!(params["tool_use_id"], "tc-claimed");
        assert_eq!(params["name"], "search");
        assert_eq!(outcome.unwrap().result.text_content(), "found");
    }

    #[tokio::test]
    async fn mob_slot_is_claimed_only_once_its_holder_closed_and_calls_follow_the_claim() {
        let (owner, owner_rx) = route_with_tool("search");
        let slot = Arc::new(MobCallbackRouteSlot::claimed_by(owner.clone()));
        let dispatcher = CallbackToolDispatcher::from_mob_slot(Arc::clone(&slot), vec![]);
        let (other, mut other_rx) = route_with_tool("search");
        let args = RawValue::from_string("{}".to_string()).unwrap();

        // An open holder is never displaced; its own claim keeps it.
        assert_eq!(
            slot.claim(&other),
            Err(MobCallbackRouteClaimRefused::HeldByOpenConnection)
        );
        assert_eq!(slot.claim(&owner), Ok(()));
        let held_binding = dispatcher.execution_binding_fingerprint("search").unwrap();

        // The holder's connection closes: the slot is released, and a call
        // fails typed instead of reaching the closed route. The released
        // route's tools stay advertised until another claim.
        drop(owner_rx);
        let released = dispatcher
            .dispatch(ToolCallView {
                id: "tc-released",
                name: "search",
                args: &args,
            })
            .await;
        assert!(is_unclaimed_route_error(&released), "{released:?}");
        assert_eq!(dispatcher.tools().len(), 1);

        // Another connection may now claim it; the claim rebinds the tool,
        // and calls go to the new holder.
        assert_eq!(slot.claim(&other), Ok(()));
        assert_ne!(
            dispatcher.execution_binding_fingerprint("search").unwrap(),
            held_binding
        );
        let call = dispatcher.dispatch(ToolCallView {
            id: "tc-reclaimed",
            name: "search",
            args: &args,
        });
        let (outcome, params) = tokio::join!(call, answer_next_callback(&mut other_rx, "other"));
        assert_eq!(params["tool_use_id"], "tc-reclaimed");
        assert_eq!(outcome.unwrap().result.text_content(), "other");
    }

    /// A detached call through a mob slot whose holder closed is refused
    /// before anything durable is written: no stored arguments and no job,
    /// so no later claim or same-named registration can run a call the
    /// model was told failed.
    #[tokio::test]
    async fn detached_call_on_a_released_mob_slot_writes_no_artifact_and_no_job() {
        let registry = Arc::new(CallbackToolRegistry::default());
        registry
            .replace_or_add_with_contracts(vec![(
                make_provenanced_tool_def("durable"),
                detached_execution_contract(),
            )])
            .unwrap();
        let (owner_tx, owner_rx) = mpsc::channel(2);
        let owner = CallbackRoute::from_parts(owner_tx, Arc::new(AtomicU64::new(0)), registry);
        let slot = Arc::new(MobCallbackRouteSlot::claimed_by(owner));
        let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
        let blob_store = Arc::new(meerkat::MemoryBlobStore::new());
        let dispatcher: Arc<dyn AgentToolDispatcher> =
            Arc::new(CallbackToolDispatcher::from_mob_slot_with_job_runtime(
                Arc::clone(&slot),
                vec![],
                "realm-a",
                job_store.clone(),
                blob_store.clone(),
            ));
        // The holder's connection closes; its detached tool stays advertised.
        drop(owner_rx);
        let args = RawValue::from_string(r#"{"target":"lan"}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "tc-released-detached",
            name: "durable",
            args: &args,
        };
        let context = meerkat_core::ToolDispatchContext::default().with_runtime_identity(
            meerkat_core::SessionId::new(),
            Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(0xdead_feed),
            )),
        );
        let resolution_context = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(30),
                ),
            ])
            .unwrap(),
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context,
        )
        .unwrap();

        let outcome =
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
                .await;
        assert!(is_unclaimed_route_error(&outcome), "{outcome:?}");
        assert!(
            meerkat::DetachedJobStore::list_all(&*job_store, 16)
                .await
                .unwrap()
                .is_empty(),
            "a refused detached call submits no job"
        );
        let arguments = meerkat_core::BlobStore::put_artifact(
            &meerkat::MemoryBlobStore::new(),
            CALLBACK_ARGUMENTS_MEDIA_TYPE,
            &standard_base64(args.get().as_bytes()),
        )
        .await
        .unwrap();
        assert!(
            meerkat_core::BlobStore::get(&*blob_store, &arguments.blob_id)
                .await
                .is_err(),
            "a refused detached call stores no arguments"
        );
    }
}
