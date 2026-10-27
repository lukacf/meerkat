//! Module lifecycle operations — registration, health checks, and capability probing.

use std::time::Duration;

use crate::runtime::{
    DeliveryHistoryRequest, DeliveryHistoryResponse, DeliveryRecord, DeliverySendError,
    DeliverySendRequest, GatingAuditEntry, GatingDecideError, GatingDecideRequest,
    GatingDecisionResult, GatingEvaluateRequest, GatingEvaluateResult, GatingPendingEntry,
    LifecycleEvent, MemoryIndexError, MemoryIndexRequest, MemoryIndexResult, MemoryQueryRequest,
    MemoryQueryResult, MemoryStoreInfo, ModuleHealthTransition, RoutingResolution,
    RoutingResolveError, RoutingResolveRequest, RuntimeMutationError, RuntimeRoute,
    RuntimeRouteMutationError, SubscribeRequest, SubscribeResponse,
};
use crate::types::{EventEnvelope, UnifiedEvent};
use crate::{ModuleRouteError, ModuleRouteRequest, ModuleRouteResponse, route_module_call};

use super::UnifiedRuntime;
use super::types::UnifiedRuntimeError;

/// Run a blocking closure on a dedicated thread to isolate it from the
/// tokio runtime. MCP boundary calls check `Handle::try_current()` and
/// refuse to block inside an active runtime, so `block_in_place` is not
/// sufficient — we need a thread that has no runtime handle at all.
///
/// Uses `std::thread::scope` so the closure can borrow from the caller.
fn run_blocking<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(f)
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

impl UnifiedRuntime {
    pub async fn module_is_running(&self) -> bool {
        self.module_runtime.lock().await.is_running()
    }

    pub async fn loaded_modules(&self) -> Vec<String> {
        self.module_runtime.lock().await.loaded_modules()
    }

    /// Reconcile modules — runs blocking subprocess I/O via `block_in_place`.
    pub async fn reconcile_modules(
        &self,
        modules: Vec<String>,
        timeout: Duration,
    ) -> Result<usize, RuntimeMutationError> {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.reconcile_modules(modules, timeout))
    }

    /// Resolve routing — runs blocking MCP boundary call via `block_in_place`.
    pub async fn resolve_routing(
        &self,
        request: RoutingResolveRequest,
    ) -> Result<RoutingResolution, RoutingResolveError> {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.resolve_routing(request))
    }

    /// Send delivery — runs blocking MCP boundary call via `block_in_place`.
    pub async fn send_delivery(
        &self,
        request: DeliverySendRequest,
    ) -> Result<DeliveryRecord, DeliverySendError> {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.send_delivery(request))
    }

    pub async fn list_runtime_routes(&self) -> Vec<RuntimeRoute> {
        self.module_runtime.lock().await.list_runtime_routes()
    }

    pub async fn add_runtime_route(
        &self,
        route: RuntimeRoute,
    ) -> Result<RuntimeRoute, RuntimeRouteMutationError> {
        self.module_runtime.lock().await.add_runtime_route(route)
    }

    pub async fn delete_runtime_route(
        &self,
        route_key: &str,
    ) -> Result<RuntimeRoute, RuntimeRouteMutationError> {
        self.module_runtime
            .lock()
            .await
            .delete_runtime_route(route_key)
    }

    pub async fn delivery_history(
        &self,
        request: DeliveryHistoryRequest,
    ) -> DeliveryHistoryResponse {
        self.module_runtime.lock().await.delivery_history(request)
    }

    pub async fn memory_stores(&self) -> Vec<MemoryStoreInfo> {
        self.module_runtime.lock().await.memory_stores()
    }

    /// Index into the memory backend. When a backend with a health-check
    /// endpoint is configured, `persist_memory_state` performs a synchronous
    /// blocking TCP healthcheck (`std::net::TcpStream::connect_timeout` +
    /// blocking socket read/write, bounded by
    /// `MEMORY_LEDGER_HEALTHCHECK_TIMEOUT`). Run it on a dedicated thread
    /// via `run_blocking` like the other blocking module ops, so the tokio
    /// worker (and every operation waiting on the `module_runtime` mutex) is
    /// not stalled for up to ~2s per call when the backend is slow/unreachable.
    pub async fn memory_index(
        &self,
        request: MemoryIndexRequest,
    ) -> Result<MemoryIndexResult, MemoryIndexError> {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.memory_index(request))
    }

    pub async fn memory_query(&self, request: MemoryQueryRequest) -> MemoryQueryResult {
        // No I/O on this path — see `memory.query` note; safe to run inline.
        self.module_runtime.lock().await.memory_query(request)
    }

    /// Evaluate a gating action — the R3 approval-notification path (and the
    /// R2/R3 memory-conflict probe) performs blocking MCP boundary calls, so
    /// the evaluation runs on a dedicated thread via `run_blocking`. Without
    /// it the approval notification silently fails with
    /// `RuntimeUnavailable("cannot execute blocking MCP boundary call inside
    /// an active tokio runtime")` and the pending request times out to
    /// safe_draft.
    pub async fn evaluate_gating_action(
        &self,
        request: GatingEvaluateRequest,
    ) -> GatingEvaluateResult {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.evaluate_gating_action(request))
    }

    /// Host-only origin-aware gating admission. The origin must come from the
    /// authenticated action context; ordinary JSON-RPC cannot supply it.
    pub async fn evaluate_gating_action_with_origin(
        &self,
        request: GatingEvaluateRequest,
        origin: Option<crate::runtime::GatingOrigin>,
    ) -> GatingEvaluateResult {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.evaluate_gating_action_with_origin(request, origin))
    }

    pub async fn list_gating_pending(&self) -> Vec<GatingPendingEntry> {
        self.module_runtime.lock().await.list_gating_pending()
    }

    pub async fn gating_state_snapshot(&self) -> crate::runtime::GatingStateSnapshot {
        self.module_runtime.lock().await.gating_state_snapshot()
    }

    pub async fn restore_gating_state(
        &self,
        snapshot: crate::runtime::GatingStateSnapshot,
    ) -> Result<(), crate::runtime::GatingStateRestoreError> {
        self.module_runtime
            .lock()
            .await
            .restore_gating_state(snapshot)
    }

    /// See [`MetaRuntimeHandle::set_gating_escalation_routes`].
    pub async fn set_gating_escalation_routes(
        &self,
        routes: crate::runtime::GatingEscalationRoutes,
    ) {
        self.module_runtime
            .lock()
            .await
            .set_gating_escalation_routes(routes);
    }

    pub async fn decide_gating_action(
        &self,
        request: GatingDecideRequest,
    ) -> Result<GatingDecisionResult, GatingDecideError> {
        self.module_runtime
            .lock()
            .await
            .decide_gating_action(request)
    }

    /// See [`MetaRuntimeHandle::gating_audit_after`].
    pub async fn gating_audit_after(
        &self,
        after_audit_id: Option<&str>,
    ) -> crate::runtime::GatingAuditPage {
        self.module_runtime
            .lock()
            .await
            .gating_audit_after(after_audit_id)
    }

    pub async fn gating_audit_entries(&self, limit: usize) -> Vec<GatingAuditEntry> {
        self.module_runtime.lock().await.gating_audit_entries(limit)
    }

    /// Spawn a module member — runs blocking subprocess I/O via `block_in_place`.
    pub async fn spawn_member(
        &self,
        module_id: &str,
        timeout: Duration,
    ) -> Result<(), RuntimeMutationError> {
        let mut rt = self.module_runtime.lock().await;
        run_blocking(|| rt.spawn_member(module_id, timeout))
    }

    /// Route a module call — runs blocking MCP boundary call via `block_in_place`.
    pub async fn route_module_call(
        &self,
        request: &ModuleRouteRequest,
        timeout: Duration,
    ) -> Result<ModuleRouteResponse, ModuleRouteError> {
        let rt = self.module_runtime.lock().await;
        run_blocking(|| route_module_call(&rt, request, timeout))
    }

    pub async fn module_lifecycle_events(&self) -> Vec<LifecycleEvent> {
        self.module_runtime.lock().await.lifecycle_events.clone()
    }

    pub async fn module_health_transitions(&self) -> Vec<ModuleHealthTransition> {
        self.module_runtime
            .lock()
            .await
            .supervisor_report
            .transitions
            .clone()
    }

    pub async fn module_events(&self) -> Vec<EventEnvelope<UnifiedEvent>> {
        self.module_runtime.lock().await.merged_events().to_vec()
    }

    pub async fn subscribe_events(
        &self,
        request: SubscribeRequest,
    ) -> Result<SubscribeResponse, UnifiedRuntimeError> {
        self.drain_mob_agent_events().await?;
        self.module_runtime
            .lock()
            .await
            .subscribe_events(request)
            .map_err(UnifiedRuntimeError::Subscribe)
    }
}
