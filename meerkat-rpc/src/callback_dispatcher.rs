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

use serde::Serialize;
use serde_json::value::RawValue;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::agent::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry};

use crate::protocol::{RpcId, RpcOutboundAdmission, RpcOutboundPermit, RpcRequest, RpcResponse};

const CALLBACK_REQUEST_MAX_PARAMS_BYTES: usize = 16 * 1024 * 1024;
const CALLBACK_REQUEST_ENVELOPE_BYTES: usize = 4 * 1024;
const CALLBACK_REQUEST_MAX_TOOL_USE_ID_BYTES: usize = 1024;
const CALLBACK_REQUEST_MAX_TOOL_NAME_BYTES: usize = 256;
pub(crate) const CALLBACK_PROCESS_MAX_PENDING: usize = 64;

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
const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Live callback-tool registry whose semantic mutation and epoch advancement
/// share one lock/authority boundary.
pub struct CallbackToolRegistry {
    tools: Arc<StdRwLock<Vec<ToolDef>>>,
    generation: Arc<AtomicU64>,
}

impl Default for CallbackToolRegistry {
    fn default() -> Self {
        Self {
            tools: Arc::new(StdRwLock::new(Vec::new())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl CallbackToolRegistry {
    pub(crate) fn replace_or_add(&self, replacements: Vec<ToolDef>) -> Result<usize, ()> {
        let mut tools = self.tools.write().map_err(|_| ())?;
        let count = replacements.len();
        for replacement in replacements {
            if let Some(existing) = tools.iter_mut().find(|tool| tool.name == replacement.name) {
                *existing = replacement;
            } else {
                tools.push(replacement);
            }
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(count)
    }

    pub fn snapshot(&self) -> Vec<ToolDef> {
        self.tools
            .read()
            .map(|tools| tools.clone())
            .unwrap_or_default()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn from_test_parts(tools: Arc<StdRwLock<Vec<ToolDef>>>, generation: Arc<AtomicU64>) -> Self {
        Self { tools, generation }
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
    /// Live tool definitions — shared with `SessionRuntime::registered_tools_slot`.
    registered_tools: Arc<CallbackToolRegistry>,
    /// Per-session inline tools (static, usually empty). Win on name collision.
    inline_tools: Vec<ToolDef>,
    /// Inline tool names cached for fast collision lookup.
    inline_names: HashSet<String>,
    callback_tx: mpsc::Sender<CallbackRequestEnvelope>,
    id_counter: Arc<AtomicU64>,
    /// Snapshot of global tool names from the last `poll_external_updates` (or creation).
    /// Used to detect additions/removals between polls.
    last_seen_globals: StdRwLock<HashSet<String>>,
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
        let inline_names: HashSet<String> =
            inline_tools.iter().map(|t| t.name.to_string()).collect();
        let last_seen_globals = registered_tools
            .tools
            .read()
            .map(|tools| {
                tools
                    .iter()
                    .filter(|t| !inline_names.contains(t.name.as_str()))
                    .map(|t| t.name.to_string())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            registered_tools,
            inline_tools,
            inline_names,
            callback_tx,
            id_counter,
            last_seen_globals: StdRwLock::new(last_seen_globals),
        }
    }

    fn next_id(&self) -> RpcId {
        let n = self.id_counter.fetch_add(1, Ordering::Relaxed);
        RpcId::Str(format!("srv-{n}"))
    }

    /// Current global tool names excluding inline collisions.
    fn current_global_names(&self) -> HashSet<String> {
        self.registered_tools
            .tools
            .read()
            .map(|tools| {
                tools
                    .iter()
                    .filter(|t| !self.inline_names.contains(t.name.as_str()))
                    .map(|t| t.name.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn is_known(&self, name: &str) -> bool {
        if self.inline_names.contains(name) {
            return true;
        }
        self.registered_tools
            .tools
            .read()
            .map(|tools| tools.iter().any(|tool| tool.name == name))
            .unwrap_or(false)
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
        if let Ok(global) = self.registered_tools.tools.read() {
            for t in global.iter() {
                if !self.inline_names.contains(t.name.as_str()) {
                    result.push(Arc::new(t.clone()));
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
        if let Ok(global) = self.registered_tools.tools.read() {
            for tool in global.iter() {
                if !self.inline_names.contains(tool.name.as_str()) {
                    result.push(callback_catalog_entry(tool.clone()));
                }
            }
        }
        result.into()
    }

    fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
        self.registered_tools.generation()
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
        if !self.is_known(call.name) {
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
        }

        let request_id = self.next_id();
        let params = CallbackParams {
            tool_use_id: call.id,
            name: call.name,
            arguments: call.args,
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

        self.callback_tx
            .send(CallbackRequestEnvelope::new(
                request,
                response_tx,
                outbound_permit,
            ))
            .await
            .map_err(|_| ToolError::execution_failed("Callback channel closed".to_string()))?;

        let response_handoff = tokio::time::timeout(CALLBACK_TIMEOUT, response_rx)
            .await
            .map_err(|_| callback_timeout_error(call.name))?
            .map_err(|_| {
                ToolError::execution_failed("Callback response channel dropped".to_string())
            })?;

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
            content: String,
            #[serde(default)]
            is_error: bool,
        }

        let parsed: CallbackResult = serde_json::from_str(result_raw.get()).map_err(|e| {
            ToolError::execution_failed(format!("Failed to parse callback result: {e}"))
        })?;
        let outcome = ToolResult::new(call.id.to_string(), parsed.content, parsed.is_error).into();
        drop(response_handoff);
        Ok(outcome)
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

fn callback_catalog_entry(tool: ToolDef) -> ToolCatalogEntry {
    let tool = Arc::new(tool);
    let entry = if let Some(provenance) = tool.provenance.clone() {
        ToolCatalogEntry::session_deferred(tool, true, provenance)
    } else {
        ToolCatalogEntry::session_inline(tool, true)
    };
    entry.with_execution_contract(meerkat_core::ToolExecutionContract::default())
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
            }),
            ..make_tool_def(name)
        }
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
    fn callback_internal_expiry_is_typed_timeout() {
        assert!(matches!(
            callback_timeout_error("slow"),
            ToolError::Timeout {
                name,
                timeout_ms: 30_000,
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
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("a")]));

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, vec![]);

        // No change yet.
        let update = dispatcher.poll_external_updates().await;
        assert!(update.notices.is_empty());

        // Add a tool.
        tools.write().unwrap().push(make_tool_def("b"));
        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:b");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);

        // Remove original tool.
        tools.write().unwrap().retain(|t| t.name != "a");
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
        let tools = Arc::new(StdRwLock::new(vec![make_tool_def("x")]));

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, vec![]);
        assert_eq!(dispatcher.tools().len(), 1);

        tools.write().unwrap().push(make_tool_def("y"));
        assert_eq!(dispatcher.tools().len(), 2);

        tools.write().unwrap().clear();
        assert_eq!(dispatcher.tools().len(), 0);
    }

    #[tokio::test]
    async fn tool_catalog_reflects_late_registered_provenanced_tools() {
        let (callback_tx, _rx) = mpsc::channel(10);
        let id_counter = Arc::new(AtomicU64::new(0));
        let tools = Arc::new(StdRwLock::new(vec![make_provenanced_tool_def(
            "secret_lookup",
        )]));

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, vec![]);
        assert_eq!(
            dispatcher
                .tool_catalog()
                .iter()
                .map(|entry| entry.tool.name.clone())
                .collect::<Vec<_>>(),
            vec!["secret_lookup".to_string()]
        );

        tools
            .write()
            .unwrap()
            .push(make_provenanced_tool_def("secret_audit"));

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
        let tools = Arc::new(StdRwLock::new(vec![]));
        let inline = vec![make_tool_def("my_inline")];

        let dispatcher =
            CallbackToolDispatcher::new(tools.clone(), callback_tx, id_counter, inline);
        assert_eq!(dispatcher.tools().len(), 1); // just inline

        // Register a global tool later.
        tools.write().unwrap().push(make_tool_def("late_global"));
        assert_eq!(dispatcher.tools().len(), 2); // inline + late_global

        let update = dispatcher.poll_external_updates().await;
        assert_eq!(update.notices.len(), 1);
        assert_eq!(update.notices[0].target, "callback:late_global");
        assert_eq!(update.notices[0].operation, ToolConfigChangeOperation::Add);
    }
}
