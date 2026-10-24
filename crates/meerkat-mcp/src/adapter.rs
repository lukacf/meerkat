//! Adapter that bridges [`McpRouter`] to [`AgentToolDispatcher`].

use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceHandle, ExternalToolSurfaceInput,
    ExternalToolSurfaceTransition, McpServerLifecycleHandle, SurfaceDiagnosticSnapshot,
    SurfaceSnapshot,
};
use meerkat_core::{
    ExternalToolDelta, ExternalToolSurfaceBaseState, ExternalToolSurfaceFailureCause,
    ExternalToolSurfacePendingOp, ExternalToolSurfaceSnapshot, ExternalToolSurfaceStagedOp,
    ExternalToolUpdate, ToolCallView, ToolCatalogCapabilities, ToolCatalogEntry, ToolDef,
    agent::AgentToolDispatcher,
};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::RwLock as AsyncRwLock;

use crate::{McpApplyResult, McpError, McpReloadTarget, McpRouter};
use meerkat_core::McpServerConfig;
use meerkat_core::mcp_attachment::McpAttachmentAuthority;

// Process-local namespace for pre-dispatch witnesses only. Router recovery or
// reconstruction must resolve a new plan rather than restore this identity.
static NEXT_MCP_EXECUTION_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

/// Typed fail-closed outcome of [`McpRouterAdapter::wait_until_ready`] when
/// MCP servers are still pending at the deadline (K14).
///
/// Callers that documented "wait until every server finishes connecting"
/// must treat this as a fault that prevents the first turn — never as a
/// warn-and-proceed.
#[derive(Debug, thiserror::Error)]
#[error("MCP servers still pending after {waited:?}: {pending:?}")]
pub struct McpNotReady {
    /// Servers still connecting when the deadline expired.
    pub pending: Vec<String>,
    /// Notices drained while waiting (completed/failed servers observed
    /// before the deadline) so they are not lost with the fault.
    pub notices: Vec<ExternalToolDelta>,
    /// How long the caller actually waited.
    pub waited: Duration,
}

/// Typed report from [`McpRouterAdapter::stage_reload_all`] (K14).
///
/// One reload primitive owns the whole-roster stage under a single router
/// lock; per-server stage rejections are typed entries, so callers surface a
/// non-clean report as a wire fault instead of `?`-aborting a hand-rolled
/// loop halfway through the roster.
#[derive(Debug, Default)]
pub struct McpReloadReport {
    /// Servers whose reload was staged (the boundary apply happens at the
    /// next turn boundary, and per-server reconnect outcomes flow through
    /// the canonical lifecycle-action channel).
    pub staged: Vec<String>,
    /// Servers whose reload stage was rejected, with the typed fault.
    pub failed: Vec<McpReloadFailure>,
}

impl McpReloadReport {
    /// True when every active server's reload was staged.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// One per-server typed stage rejection inside an [`McpReloadReport`].
#[derive(Debug)]
pub struct McpReloadFailure {
    pub server: String,
    pub error: McpError,
}

/// What an [`McpRouterShutdownObserver`] read of its adapter's router
/// shutdown.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum McpRouterShutdownObservation {
    /// The router's shutdown finished, with what every close it owned
    /// observed.
    Reported(Arc<crate::McpRouterShutdownReport>),
    /// The shutdown can no longer report: the adapter was dropped without
    /// shutting its router down, or the shutdown task was dropped before it
    /// reported (its runtime shut down).
    Abandoned,
}

/// The one shutdown of an [`McpRouterAdapter`]'s router, observed from
/// outside the adapter ([`McpRouterAdapter::shutdown_observer`]). It outlives
/// the adapter, and every clone observes the same shutdown.
#[derive(Debug, Clone)]
pub struct McpRouterShutdownObserver {
    report: tokio::sync::watch::Receiver<Option<Arc<crate::McpRouterShutdownReport>>>,
}

impl McpRouterShutdownObserver {
    /// Wait until the shutdown reports, or can no longer report.
    pub async fn observe(&mut self) -> McpRouterShutdownObservation {
        match self.report.wait_for(Option::is_some).await {
            Ok(published) => published
                .as_ref()
                .map_or(McpRouterShutdownObservation::Abandoned, |report| {
                    McpRouterShutdownObservation::Reported(Arc::clone(report))
                }),
            Err(_) => McpRouterShutdownObservation::Abandoned,
        }
    }

    /// The observation now, without waiting: `None` while the shutdown has
    /// neither reported nor been abandoned.
    #[must_use]
    pub fn current(&self) -> Option<McpRouterShutdownObservation> {
        let reported = || {
            self.report
                .borrow()
                .as_ref()
                .map(|report| McpRouterShutdownObservation::Reported(Arc::clone(report)))
        };
        if let Some(reported) = reported() {
            return Some(reported);
        }
        // A closed channel publishes nothing more; a report sent just before
        // it closed is read again first.
        self.report
            .has_changed()
            .is_err()
            .then(|| reported().unwrap_or(McpRouterShutdownObservation::Abandoned))
    }
}

/// Adapter that wraps an [`McpRouter`] to implement [`AgentToolDispatcher`].
///
/// Tool visibility and routing come from the router's atomically published
/// projection snapshot. The adapter keeps a best-effort fallback copy so
/// `tools()` can stay non-blocking under lock contention.
pub struct McpRouterAdapter {
    router: AsyncRwLock<Option<McpRouter>>,
    has_pending: AtomicBool,
    tools_cache: StdRwLock<Arc<[Arc<ToolDef>]>>,
    catalog_cache: StdRwLock<Arc<[ToolCatalogEntry]>>,
    pending_sources_cache: StdRwLock<Arc<[String]>>,
    surface_snapshot_cache: StdRwLock<Option<ExternalToolSurfaceSnapshot>>,
    /// Shared handle slot for the session's MCP server lifecycle DSL handle
    /// (Phase 5G / T5g). Cloned from the inner router at adapter construction
    /// so the sync `AgentToolDispatcher::bind_mcp_server_lifecycle_handle`
    /// trait method can write into it without acquiring the async router
    /// lock.
    mcp_lifecycle_handle: Arc<StdRwLock<Option<Arc<dyn McpServerLifecycleHandle>>>>,
    /// Shared handle slot for the external-tool surface DSL owner. Routers
    /// constructed without a generated owner fail closed until
    /// runtime-backed session builds replace the slot with the session-owned
    /// handle.
    external_surface_handle: Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>>,
    /// The lineages the router staged on its surface. A rebind replaces
    /// them with the lineages it re-derives on the new surface.
    held_surface_lineages: crate::HeldSurfaceLineages,
    /// How the router holds its surface. A rebind onto another owner's
    /// surface (a session registration's) makes it `Borrowed`.
    surface_ownership: crate::SurfaceOwnershipSlot,
    execution_authority_id: u64,
    execution_binding_epoch: AtomicU64,
    /// Publishes the router's shutdown report to every observer
    /// ([`Self::shutdown_observer`]). Created with the adapter; the sender
    /// moves into the shutdown task when the router is taken, so a later
    /// shutdown joins that same work after an earlier call was dropped. A
    /// sender dropped without a report (the adapter dropped before shutting
    /// its router down, or the shutdown task dropped) reads as Abandoned.
    shutdown_report: std::sync::Mutex<
        Option<tokio::sync::watch::Sender<Option<Arc<crate::McpRouterShutdownReport>>>>,
    >,
    shutdown_observer: McpRouterShutdownObserver,
    /// The router's close index, outside the router lock
    /// ([`Self::begin_close`]). Once it is closing, dispatch and every
    /// staging entry refuse before taking a router guard.
    close_index: crate::connection::CloseIndex,
    /// The router's construction identity, outside the router lock.
    construction: crate::McpConstructionId,
    /// Test-only acknowledgement sent just before a bind's first read of the
    /// surface handle slot (it reads the slot, then claims it to write).
    #[cfg(test)]
    bind_claim_attempt: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

/// Fail-closed MCP server-lifecycle mirror installed when the pre-bind seed
/// replay is rejected by the session DSL (K14).
///
/// The seeded pre-bind pending servers stay visibly pending (so the
/// `[MCP_PENDING]` notice never goes silently quiet on a diverged mirror)
/// and every subsequent lifecycle apply returns the typed seed rejection,
/// which the router's notify paths propagate fail-closed.
struct PoisonedMcpServerLifecycleHandle {
    reason: DslTransitionError,
    pending: BTreeSet<String>,
}

impl PoisonedMcpServerLifecycleHandle {
    fn reject(&self, context: &'static str) -> DslTransitionError {
        DslTransitionError::guard_rejected(
            context,
            format!(
                "MCP lifecycle mirror is poisoned by a rejected bind seed: {}",
                self.reason
            ),
        )
    }
}

impl McpServerLifecycleHandle for PoisonedMcpServerLifecycleHandle {
    fn apply_connect_pending(&self, _server_id: &str) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedMcpServerLifecycleHandle::apply_connect_pending"))
    }

    fn apply_connected(&self, _server_id: &str) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedMcpServerLifecycleHandle::apply_connected"))
    }

    fn apply_failed(&self, _server_id: &str, _error: &str) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedMcpServerLifecycleHandle::apply_failed"))
    }

    fn apply_disconnected(&self, _server_id: &str) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedMcpServerLifecycleHandle::apply_disconnected"))
    }

    fn apply_reload(&self, _server_id: &str) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedMcpServerLifecycleHandle::apply_reload"))
    }

    fn pending_server_ids(&self) -> BTreeSet<String> {
        self.pending.clone()
    }
}

struct PoisonedExternalToolSurfaceHandle {
    reason: String,
    snapshot: SurfaceDiagnosticSnapshot,
}

impl PoisonedExternalToolSurfaceHandle {
    fn new(reason: String, snapshot: ExternalToolSurfaceSnapshot) -> Self {
        let entries = snapshot
            .entries
            .into_iter()
            .map(|entry| SurfaceSnapshot {
                surface_id: entry.surface_id,
                base_state: Some(entry.base_state),
                pending_op: entry.pending_op,
                staged_op: entry.staged_op,
                staged_intent_sequence: Some(entry.staged_intent_sequence),
                pending_task_sequence: Some(entry.pending_task_sequence),
                pending_lineage_sequence: Some(entry.pending_lineage_sequence),
                inflight_calls: entry.inflight_call_count,
                last_delta_operation: Some(entry.last_delta_operation),
                last_delta_phase: Some(entry.last_delta_phase),
                removal_draining_since_ms: None,
                removal_timeout_at_ms: None,
                removal_applied_at_turn: None,
            })
            .collect::<Vec<_>>();
        let known_surfaces = entries
            .iter()
            .map(|entry| entry.surface_id.clone())
            .collect::<BTreeSet<_>>();
        let visible_surfaces = entries
            .iter()
            .filter(|entry| matches!(entry.base_state, Some(ExternalToolSurfaceBaseState::Active)))
            .map(|entry| entry.surface_id.clone())
            .collect::<BTreeSet<_>>();
        let has_pending_or_staged = entries.iter().any(|entry| {
            entry.pending_op != ExternalToolSurfacePendingOp::None
                || entry.staged_op != ExternalToolSurfaceStagedOp::None
        });
        Self {
            reason,
            snapshot: SurfaceDiagnosticSnapshot {
                surface_phase: snapshot.phase,
                known_surfaces,
                visible_surfaces,
                snapshot_epoch: snapshot.snapshot_epoch,
                snapshot_aligned_epoch: snapshot.snapshot_aligned_epoch,
                has_pending_or_staged,
                entries,
            },
        }
    }

    fn reject(&self, context: &'static str) -> DslTransitionError {
        DslTransitionError::guard_rejected(
            context,
            format!(
                "external tool surface handle bind failed; surface is poisoned: {}",
                self.reason
            ),
        )
    }
}

impl ExternalToolSurfaceHandle for PoisonedExternalToolSurfaceHandle {
    fn apply_surface_input(
        &self,
        _input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::apply_surface_input"))
    }

    fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::register"))
    }

    fn stage_add(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::stage_add"))
    }

    fn stage_remove(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::stage_remove"))
    }

    fn stage_reload(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::stage_reload"))
    }

    fn apply_boundary(
        &self,
        _surface_id: String,
        _now_ms: u64,
        _staged_intent_sequence: u64,
        _applied_at_turn: u64,
    ) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::apply_boundary"))
    }

    fn mark_pending_succeeded(
        &self,
        _surface_id: String,
        _pending_task_sequence: u64,
        _staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::mark_pending_succeeded"))
    }

    fn mark_pending_failed(
        &self,
        _surface_id: String,
        _pending_task_sequence: u64,
        _staged_intent_sequence: u64,
        _cause: ExternalToolSurfaceFailureCause,
    ) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::mark_pending_failed"))
    }

    fn call_started(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::call_started"))
    }

    fn call_finished(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::call_finished"))
    }

    fn finalize_removal_clean(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::finalize_removal_clean"))
    }

    fn finalize_removal_forced(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::finalize_removal_forced"))
    }

    fn snapshot_aligned(&self, _epoch: u64) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::snapshot_aligned"))
    }

    fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
        Err(self.reject("PoisonedExternalToolSurfaceHandle::shutdown_surface"))
    }

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
        self.snapshot
            .entries
            .iter()
            .find(|entry| entry.surface_id == surface_id)
            .cloned()
    }

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
        self.snapshot.clone()
    }

    fn visible_surfaces(&self) -> BTreeSet<String> {
        self.snapshot.visible_surfaces.clone()
    }

    fn removing_surfaces(&self) -> BTreeSet<String> {
        self.snapshot
            .entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.base_state,
                    Some(ExternalToolSurfaceBaseState::Removing)
                )
            })
            .map(|entry| entry.surface_id.clone())
            .collect()
    }

    fn pending_surfaces(&self) -> BTreeSet<String> {
        self.snapshot
            .entries
            .iter()
            .filter(|entry| entry.pending_op != ExternalToolSurfacePendingOp::None)
            .map(|entry| entry.surface_id.clone())
            .collect()
    }

    fn has_pending_or_staged(&self) -> bool {
        self.snapshot.has_pending_or_staged
    }

    fn snapshot_epoch(&self) -> u64 {
        self.snapshot.snapshot_epoch
    }

    fn snapshot_aligned_epoch(&self) -> u64 {
        self.snapshot.snapshot_aligned_epoch
    }
}

impl McpRouterAdapter {
    pub fn new(router: McpRouter) -> Self {
        let has_pending = router.has_pending_or_notices();
        let tools = AgentToolDispatcher::tools(&router);
        let catalog = AgentToolDispatcher::tool_catalog(&router);
        let pending_sources: Arc<[String]> = router.pending_sources_snapshot().into();
        let surface_snapshot = McpRouter::external_tool_surface_snapshot(&router);
        let mcp_lifecycle_handle = router.mcp_lifecycle_handle_slot();
        let external_surface_handle = router.external_surface_handle_slot();
        let held_surface_lineages = router.held_surface_lineages();
        let surface_ownership = router.surface_ownership_slot();
        let close_index = router.close_index();
        let construction = router.construction_id();
        let (shutdown_report, report) = tokio::sync::watch::channel(None);
        Self {
            router: AsyncRwLock::new(Some(router)),
            has_pending: AtomicBool::new(has_pending),
            tools_cache: StdRwLock::new(tools),
            catalog_cache: StdRwLock::new(catalog),
            pending_sources_cache: StdRwLock::new(pending_sources),
            surface_snapshot_cache: StdRwLock::new(Some(surface_snapshot)),
            mcp_lifecycle_handle,
            external_surface_handle,
            held_surface_lineages,
            surface_ownership,
            execution_authority_id: NEXT_MCP_EXECUTION_AUTHORITY_ID.fetch_add(1, Ordering::Relaxed),
            execution_binding_epoch: AtomicU64::new(0),
            shutdown_report: std::sync::Mutex::new(Some(shutdown_report)),
            shutdown_observer: McpRouterShutdownObserver { report },
            close_index,
            construction,
            #[cfg(test)]
            bind_claim_attempt: std::sync::Mutex::new(None),
        }
    }

    /// The construction identity of this adapter's router
    /// ([`McpRouter::construction_id`]): the
    /// [`crate::McpAttemptKey::construction`] of its attempts.
    #[must_use]
    pub fn construction_id(&self) -> crate::McpConstructionId {
        self.construction
    }

    fn execution_authority_key(&self) -> String {
        format!("mcp-router-adapter:{}", self.execution_authority_id)
    }

    /// The router this adapter owns, for a checked route over it.
    pub(crate) fn router_lock(&self) -> &AsyncRwLock<Option<McpRouter>> {
        &self.router
    }

    /// A trusted host's checked route over this adapter's router to exactly
    /// `spec.operations` of `spec.server`, registered under the stable name
    /// `spec.route`, for `session`: the construction's actual session, as
    /// the factory passes it to its MCP adapter composer. The route offers
    /// those operations under their genuine definitions even when the
    /// server's access ceiling excludes them, and runs only plans it resolved
    /// itself, under actual bound work and a prepared authorization for that
    /// session, once per resolved plan, while it is open; the ordinary
    /// projection and every other entry keep the ceiling. Creating it grants
    /// nothing: each call still needs the work's grant selected for exactly
    /// this route to this server, and that grant is route authority only,
    /// never proof that the host's own checks ran. Hand the route only to
    /// the host's own checked wrapper; it is never a session tool surface of
    /// its own.
    ///
    /// # Errors
    /// An empty route name, server name or operation, or no operation.
    pub fn checked_route(
        self: &Arc<Self>,
        session: meerkat_core::types::SessionId,
        spec: crate::CheckedMcpRouteSpec,
    ) -> Result<Arc<crate::CheckedMcpRoute>, crate::CheckedMcpRouteError> {
        crate::CheckedMcpRoute::new(Arc::clone(self), session, spec).map(Arc::new)
    }

    fn live_binding_fingerprint(
        &self,
        router: &McpRouter,
        tool_name: &str,
    ) -> Option<meerkat_core::EphemeralToolBindingFingerprint> {
        AgentToolDispatcher::tool_catalog(router)
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .map(meerkat_core::ephemeral_tool_catalog_binding_fingerprint)
            .map(|fingerprint| {
                fingerprint.with_live_authority(
                    self.execution_authority_id as usize,
                    self.execution_binding_epoch.load(Ordering::Acquire),
                )
            })
    }

    fn owner_witness(
        &self,
        router: &McpRouter,
        tool_name: &str,
    ) -> Result<meerkat_core::ToolExecutionOwnerWitness, meerkat_core::ToolExecutionResolutionError>
    {
        let fingerprint = self
            .live_binding_fingerprint(router, tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            self.execution_authority_key(),
            tool_name,
            fingerprint,
        )
        .map_err(meerkat_core::ToolExecutionResolutionError::from)?;
        // The exact server registration and raw operation, under the same
        // router read: the operation owner's fact, revalidated at dispatch.
        Ok(match router.operation_target(tool_name) {
            Some(target) => witness.with_operation_target(target),
            None => witness,
        })
    }

    /// The plan's operation target is still the one this live router
    /// resolves the call to (same server registration, connection
    /// generation and raw operation).
    fn operation_target_matches(
        &self,
        router: &McpRouter,
        tool_name: &str,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> bool {
        plan.owner_witness(&self.execution_authority_key())
            .is_some_and(|witness| {
                witness.operation_target() == router.operation_target(tool_name).as_ref()
            })
    }

    fn binding_witness_matches(
        &self,
        tool_name: &str,
        live_fingerprint: Option<meerkat_core::EphemeralToolBindingFingerprint>,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> bool {
        plan.owner_witness(&self.execution_authority_key())
            .is_some_and(|witness| {
                witness.owner_key() == tool_name
                    && live_fingerprint.as_ref() == Some(witness.binding_fingerprint())
            })
    }

    fn set_tools_cache(&self, tools: Arc<[Arc<ToolDef>]>) {
        match self.tools_cache.write() {
            Ok(mut guard) => *guard = tools,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = tools;
            }
        }
    }

    fn cached_tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self.tools_cache.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn set_catalog_cache(&self, catalog: Arc<[ToolCatalogEntry]>) {
        match self.catalog_cache.write() {
            Ok(mut guard) => *guard = catalog,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = catalog;
            }
        }
    }

    fn set_surface_snapshot_cache(&self, snapshot: Option<ExternalToolSurfaceSnapshot>) {
        match self.surface_snapshot_cache.write() {
            Ok(mut guard) => *guard = snapshot,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = snapshot;
            }
        }
    }

    fn cached_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        match self.catalog_cache.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn set_pending_sources_cache(&self, pending_sources: Arc<[String]>) {
        match self.pending_sources_cache.write() {
            Ok(mut guard) => *guard = pending_sources,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = pending_sources;
            }
        }
    }

    fn cached_pending_sources(&self) -> Arc<[String]> {
        match self.pending_sources_cache.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn cached_surface_snapshot(&self) -> Option<ExternalToolSurfaceSnapshot> {
        match self.surface_snapshot_cache.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn sync_router_projection(&self, router: &McpRouter) {
        self.set_tools_cache(AgentToolDispatcher::tools(router));
        self.set_catalog_cache(AgentToolDispatcher::tool_catalog(router));
        self.set_pending_sources_cache(router.pending_sources_snapshot().into());
        self.has_pending
            .store(router.has_pending_or_notices(), Ordering::Release);
        self.set_surface_snapshot_cache(Some(McpRouter::external_tool_surface_snapshot(router)));
    }

    /// Refresh projection state from the router.
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let router = self.router.read().await;
        if let Some(router) = router.as_ref() {
            self.sync_router_projection(router);
        }
        Ok(())
    }

    /// What this adapter's router exposes now for one exact raw operation;
    /// `Retired` once the router has been taken for shutdown.
    pub async fn observe_operation(
        &self,
        server: &str,
        raw_operation: &str,
    ) -> meerkat_core::McpOperationObservation {
        match self.router.read().await.as_ref() {
            Some(router) => router.observe_operation(server, raw_operation),
            None => meerkat_core::McpOperationObservation::Retired,
        }
    }

    /// Gracefully shutdown the MCP router.
    ///
    /// [`Self::shutdown_with_report`], discarding the observation.
    pub async fn shutdown(&self) {
        let _ = self.shutdown_with_report().await;
    }

    /// Gracefully shut down the MCP router, and return what its shutdown
    /// observed of every close it owned ([`crate::McpRouter::shutdown_with_report`]).
    ///
    /// Takes the router out of the adapter and shuts it down in a task of its
    /// own, so dropping this call stops only the wait: the taken router still
    /// finishes its shutdown, and a later call waits for that same shutdown
    /// and reads the same report. The router lock is released once the
    /// router is taken, so a reader (an attempt guard's finish among them)
    /// never waits for the shutdown: it finds the router gone. After this
    /// call, tool calls will fail.
    pub async fn shutdown_with_report(&self) -> McpRouterShutdownObservation {
        // Every close starts now, without the router lock: an in-flight call
        // that holds a router guard does not delay it.
        drop(self.begin_close());
        let taken = {
            let mut router = self.router.write().await;
            let taken = router.take();
            if taken.is_some() {
                self.execution_binding_epoch.fetch_add(1, Ordering::AcqRel);
            }
            taken
        };
        if let Some(router) = taken {
            let report = self
                .shutdown_report
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            tokio::spawn(async move {
                let shutdown = router.shutdown_with_report().await;
                if let Some(report) = report {
                    report.send_replace(Some(Arc::new(shutdown)));
                }
            });
        }
        let observation = self.shutdown_observer().observe().await;
        if matches!(observation, McpRouterShutdownObservation::Abandoned) {
            tracing::error!("MCP router shutdown task ended without finishing");
        }
        self.has_pending.store(false, Ordering::Release);
        self.set_tools_cache(Arc::from([]));
        self.set_pending_sources_cache(Arc::from([]));
        self.set_surface_snapshot_cache(None);
        self.set_catalog_cache(Arc::from([]));
        self.set_pending_sources_cache(Arc::from([]));
        self.set_surface_snapshot_cache(None);
        observation
    }

    /// Close admission and start, now, every close the router owns, without
    /// the router lock or any in-flight dispatch: from here on dispatch and
    /// every staging entry refuse before taking a router guard, a connect
    /// attempt stops and spawns or connects nothing more, each held stdio
    /// server's client is closed and then its process under its own
    /// bounds, and each remote connection's client is closed. A close
    /// already running is joined. A later call returns a ticket over the
    /// same closes, and [`Self::shutdown_with_report`] joins them for the
    /// same outcomes. Call it within a Tokio runtime.
    pub fn begin_close(&self) -> crate::McpCloseTicket {
        self.close_index.begin_close()
    }

    /// Whether the router's close began ([`Self::begin_close`]).
    pub(crate) fn is_closing(&self) -> bool {
        self.close_index.is_closing()
    }

    fn refuse_if_closing(&self) -> Result<(), String> {
        if self.is_closing() {
            return Err("MCP router is closing".to_string());
        }
        Ok(())
    }

    /// An observer of this adapter's one router shutdown, whoever starts it.
    /// It outlives the adapter: dropped without shutting its router down, the
    /// adapter leaves its observers reading
    /// [`McpRouterShutdownObservation::Abandoned`].
    #[must_use]
    pub fn shutdown_observer(&self) -> McpRouterShutdownObserver {
        self.shutdown_observer.clone()
    }

    /// Read collision diagnostics from the router's current projection owner.
    /// No separate adapter cache or naming authority is maintained.
    pub async fn tool_name_collisions(
        &self,
    ) -> Result<Arc<[crate::McpToolNameCollision]>, crate::McpError> {
        self.router
            .read()
            .await
            .as_ref()
            .map(McpRouter::tool_name_collisions)
            .ok_or(crate::McpError::RouterShutDown)
    }

    /// Stage an MCP server add operation.
    pub async fn stage_add(&self, config: McpServerConfig) -> Result<(), String> {
        self.refuse_if_closing()?;
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .stage_add(config)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    /// Stage the add of a server the host refused before anything in it was
    /// expanded, authorized, connected or spawned
    /// ([`McpRouter::stage_add_refused`]): applying it reports the typed
    /// `refusal` as that server's failed setup, and nothing connects.
    pub async fn stage_add_refused(
        &self,
        config: McpServerConfig,
        refusal: meerkat_core::confinement::ConfinementRefusal,
    ) -> Result<(), String> {
        self.refuse_if_closing()?;
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .stage_add_refused(config, refusal)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    /// Stage a governed MCP server add under a prepared attachment control.
    ///
    /// The router installs the exact descriptor and its control together,
    /// under one borrow; the control is for this descriptor only and is asked
    /// once, at the connection's physical entry after preparation. A refusal
    /// there fails that connection before any spawn or request.
    pub async fn stage_add_governed(
        &self,
        config: McpServerConfig,
        authority: Arc<dyn McpAttachmentAuthority>,
    ) -> Result<(), String> {
        self.refuse_if_closing()?;
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .stage_add_governed(config, authority)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    /// [`Self::stage_add_governed`], returning the exact attempt this add
    /// starts, observed against this adapter's own router (see
    /// [`meerkat_core::mcp_attachment::McpAttachmentAttempt`]). The attempt
    /// holds this adapter weakly: it is never a strong owner of the router (a
    /// read upgrades it only for that read) and never owns its cleanup.
    pub async fn stage_add_governed_observed(
        self: &Arc<Self>,
        config: McpServerConfig,
        authority: Arc<dyn McpAttachmentAuthority>,
    ) -> Result<meerkat_core::mcp_attachment::McpAttachmentAttempt, String> {
        self.refuse_if_closing()?;
        let server = config.name.clone();
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let state = router
            .stage_add_governed_attempt(config, authority)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(meerkat_core::mcp_attachment::McpAttachmentAttempt::new(
            server.clone(),
            Arc::new(crate::attempt::RouterAttemptObserver::new(
                Arc::downgrade(self),
                server,
                state,
            )),
        ))
    }

    /// Process arrived connection results as lifecycle polling does, leaving
    /// their notices queued for the normal delivery, and return the wait for
    /// the router's next progress (taken before processing, so a result
    /// delivered meanwhile wakes it). `None` once the router is gone.
    pub(crate) async fn progress_pending(&self) -> Option<crate::router::McpProgressWait> {
        let mut router = self.router.write().await;
        let router = router.as_mut()?;
        let next = router.progress_wait();
        router.progress_pending();
        self.sync_router_projection(router);
        Some(next)
    }

    /// See [`McpRouter::installed_operation`]; `None` once the router is
    /// gone.
    pub(crate) async fn installed_operation(
        &self,
        server: &str,
        connection_generation: u64,
        tool: &str,
    ) -> Option<crate::router::InstalledOperation> {
        self.router
            .read()
            .await
            .as_ref()
            .map(|router| router.installed_operation(server, connection_generation, tool))
    }

    /// Stage an MCP server remove operation.
    pub async fn stage_remove(&self, server_name: impl Into<String>) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .stage_remove(server_name)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    /// List names of all active (non-removed) servers.
    pub async fn active_server_names(&self) -> Vec<String> {
        let router = self.router.read().await;
        match router.as_ref() {
            Some(r) => r.active_server_names(),
            None => Vec::new(),
        }
    }

    /// Host-channel status: MCP targets waiting for human OAuth authorization
    /// through the host. See [`McpRouter::servers_awaiting_authorization`].
    /// Not an agent event; it carries no authorize URL, state or code.
    pub async fn servers_awaiting_authorization(
        &self,
    ) -> Vec<meerkat_auth_core::McpServerIdentity> {
        let router = self.router.read().await;
        match router.as_ref() {
            Some(r) => r.servers_awaiting_authorization(),
            None => Vec::new(),
        }
    }

    /// Host status: the typed setup failure of `server`'s latest failed
    /// stdio attempt that this router still retains
    /// ([`meerkat_core::mcp_attachment::McpStdioSetupFailure`]): the failure
    /// behind that server's latest `Failed` lifecycle notice. Host Rust API
    /// only; it carries no server text.
    ///
    /// The router retains at most one failed attempt per server, from its
    /// accepted failure until the next attempt for the server is admitted,
    /// the next accepted completion, its removal, its connection's close,
    /// shutdown, or the router's drop; `None` after any of those, or when
    /// the latest failure was not a classified stdio setup failure. It
    /// reflects results already drained by the normal lifecycle polling and
    /// drains nothing itself.
    pub async fn stdio_setup_failure(
        &self,
        server: &str,
    ) -> Option<meerkat_core::mcp_attachment::McpStdioSetupFailure> {
        self.router
            .read()
            .await
            .as_ref()?
            .stdio_setup_failure(server)
    }

    /// Host status: the bounded, redacted stderr tail of the same retained
    /// attempt as [`Self::stdio_setup_failure`], under the same retention.
    ///
    /// SENSITIVE: host and operator only, never model context, events or
    /// untrusted clients; redaction is defence in depth only
    /// ([`meerkat_core::mcp_attachment::McpStdioStderrTail`]).
    pub async fn stdio_stderr_tail(
        &self,
        server: &str,
    ) -> Option<meerkat_core::mcp_attachment::McpStdioStderrTail> {
        self.router
            .read()
            .await
            .as_ref()?
            .stdio_stderr_tail(server, None)
    }

    /// The retained stderr tail of `server`'s exact failed `attempt`, for
    /// that attempt's observer; `None` once released or the router is gone.
    pub(crate) async fn stdio_stderr_tail_of(
        &self,
        server: &str,
        attempt: u64,
    ) -> Option<meerkat_core::mcp_attachment::McpStdioStderrTail> {
        self.router
            .read()
            .await
            .as_ref()?
            .stdio_stderr_tail(server, Some(attempt))
    }

    /// Whether the router still retains the stderr tail of `server`'s exact
    /// failed `attempt`.
    pub(crate) async fn retains_stdio_tail(&self, server: &str, attempt: u64) -> bool {
        self.router
            .read()
            .await
            .as_ref()
            .is_some_and(|router| router.retains_stdio_tail(server, attempt))
    }

    /// After a successful login for `target`, stage again the failed
    /// operation awaiting exactly that target ([`McpRouter::retry_authorized`]).
    /// It takes the router write lock that shutdown takes, so a router
    /// already shut down is
    /// [`AuthorizedRetry::RouterClosed`](crate::AuthorizedRetry::RouterClosed)
    /// and nothing is reinstalled from a detached adapter.
    pub async fn retry_authorized(
        &self,
        target: &meerkat_auth_core::McpServerIdentity,
    ) -> Result<crate::AuthorizedRetry, crate::McpError> {
        if self.is_closing() {
            return Ok(crate::AuthorizedRetry::RouterClosed);
        }
        let mut router = self.router.write().await;
        let Some(router) = router.as_mut() else {
            return Ok(crate::AuthorizedRetry::RouterClosed);
        };
        let retried = router.retry_authorized(target);
        self.sync_router_projection(router);
        retried
    }

    /// After the credential for `target` was released (signed out), stop
    /// offering every connected server that uses exactly it
    /// ([`McpRouter::credential_released`]). It takes the router write lock
    /// that shutdown takes, so a router already shut down is
    /// [`crate::ReleasedCredential::RouterClosed`].
    pub async fn credential_released(
        &self,
        target: &meerkat_auth_core::McpServerIdentity,
    ) -> Result<crate::ReleasedCredential, crate::McpError> {
        let mut router = self.router.write().await;
        let Some(router) = router.as_mut() else {
            return Ok(crate::ReleasedCredential::RouterClosed);
        };
        let released = router.credential_released(target);
        self.sync_router_projection(router);
        released
    }

    /// Stage an MCP server reload operation.
    pub async fn stage_reload<T: Into<McpReloadTarget>>(&self, target: T) -> Result<(), String> {
        self.refuse_if_closing()?;
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .stage_reload(target)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    /// Apply staged MCP operations.
    pub async fn apply_staged(&self) -> Result<McpApplyResult, String> {
        self.refuse_if_closing()?;
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let result = router
            .apply_staged()
            .await
            .map_err(|error| error.to_string())?;
        self.execution_binding_epoch.fetch_add(1, Ordering::AcqRel);
        self.sync_router_projection(router);
        Ok(result)
    }

    /// Drain background lifecycle completions as canonical MCP lifecycle actions.
    pub async fn poll_lifecycle_actions(&self) -> Result<Vec<crate::McpLifecycleAction>, String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let actions = router.take_lifecycle_actions();
        if !actions.is_empty() {
            self.execution_binding_epoch.fetch_add(1, Ordering::AcqRel);
        }
        self.sync_router_projection(router);
        Ok(actions)
    }

    /// Drive draining (Removing) servers to finalization in a background
    /// task, forwarding their lifecycle actions to `lifecycle_tx`.
    ///
    /// `running` guards one drain per adapter: a call while a drain runs is a
    /// no-op. The drain is woken by typed progress, never a timer poll: a
    /// finished tool call (a draining server's in-flight count drops) or the
    /// earliest removal timeout of a draining server. It ends when no server
    /// is draining, and reclaims the drain for a removal staged between that
    /// check and releasing `running`.
    pub fn spawn_removal_drain(
        self: &Arc<Self>,
        running: Arc<AtomicBool>,
        lifecycle_tx: tokio::sync::mpsc::UnboundedSender<crate::McpLifecycleAction>,
    ) {
        if running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let adapter = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let next = match adapter.progress_removals_and_next_wait(&lifecycle_tx).await {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::warn!("background MCP drain apply failed: {error}");
                        running.store(false, Ordering::Release);
                        return;
                    }
                };
                if let Some(wait) = next {
                    wait.wait(None).await;
                    continue;
                }
                running.store(false, Ordering::Release);
                // A removal staged after the check above saw `running` set and
                // spawned no drain of its own.
                match adapter.has_removing_servers().await {
                    Ok(true)
                        if running
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok() => {}
                    _ => return,
                }
            }
        });
    }

    /// [`Self::progress_removals`], forwarding its lifecycle actions, plus
    /// what the drain waits on next. All under one router lock: whoever then
    /// observes a removal finalized also finds its action queued, and no
    /// progress between the pass and the wait is missed.
    async fn progress_removals_and_next_wait(
        &self,
        lifecycle_tx: &tokio::sync::mpsc::UnboundedSender<crate::McpLifecycleAction>,
    ) -> Result<Option<crate::router::McpProgressWait>, String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let delta = router
            .progress_removals()
            .await
            .map_err(|e| e.to_string())?;
        self.sync_router_projection(router);
        for action in delta.lifecycle_actions {
            let _ = lifecycle_tx.send(action);
        }
        Ok(router.removal_progress_wait())
    }

    /// Wait until every spawned connect attempt has delivered its result to
    /// the router, without consuming the results: the next boundary still
    /// processes them and emits their lifecycle actions. Returns `false` if
    /// `limit` elapses first (a hang guard for tests).
    #[cfg(any(test, feature = "test-support"))]
    pub async fn wait_connect_results_delivered(&self, limit: Duration) -> bool {
        let limit = tokio::time::Instant::now() + limit;
        loop {
            let wait = {
                let router = self.router.read().await;
                let Some(router) = router.as_ref() else {
                    return true;
                };
                let wait = router.progress_wait();
                if !router.connect_results_outstanding() {
                    return true;
                }
                wait
            };
            if tokio::time::Instant::now() >= limit {
                return false;
            }
            wait.wait(Some(limit)).await;
        }
    }

    /// Wait until no server is draining (every removal finalized by the
    /// drain). Returns `false` if `limit` elapses first (a hang guard for
    /// tests).
    #[cfg(any(test, feature = "test-support"))]
    pub async fn wait_removals_finalized(&self, limit: Duration) -> bool {
        let limit = tokio::time::Instant::now() + limit;
        loop {
            let wait = {
                let router = self.router.read().await;
                let Some(router) = router.as_ref() else {
                    return true;
                };
                let wait = router.progress_wait();
                if !router.has_removing_servers() {
                    return true;
                }
                wait
            };
            if tokio::time::Instant::now() >= limit {
                return false;
            }
            wait.wait(Some(limit)).await;
        }
    }

    /// Progress only Removing server finalization (drain/timeout) without applying staged ops.
    pub async fn progress_removals(&self) -> Result<crate::McpApplyDelta, String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let delta = router
            .progress_removals()
            .await
            .map_err(|e| e.to_string())?;
        // Only reflect the projection after a committed finalize (fail closed:
        // on a rejected removal the projection must NOT advance).
        self.sync_router_projection(router);
        Ok(delta)
    }

    /// Returns true if any MCP server is currently draining in Removing state.
    pub async fn has_removing_servers(&self) -> Result<bool, String> {
        let router = self.router.read().await;
        let router = router
            .as_ref()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        Ok(router.has_removing_servers())
    }

    /// Block until all pending MCP connections complete or the timeout
    /// expires.
    ///
    /// Returns notices for completed/failed servers. Useful for CLI
    /// `--wait-for-mcp` and SDK `wait_for_mcp` workflows where the caller
    /// needs tools to be available before the first agent turn.
    ///
    /// K14: the deadline is fail-closed — if servers are still pending when
    /// it expires, the typed [`McpNotReady`] fault (carrying the pending set
    /// and the notices drained so far) is returned so callers abort the
    /// first turn instead of warn-and-proceeding into it.
    pub async fn wait_until_ready(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Vec<ExternalToolDelta>, McpNotReady> {
        let mut all_notices = Vec::new();
        let started = tokio::time::Instant::now();
        let deadline = started + timeout;
        loop {
            // Taken before the poll: a connect result delivered after the
            // poll read the pending set wakes the wait.
            let next = self
                .router
                .read()
                .await
                .as_ref()
                .map(McpRouter::progress_wait);
            let update = self.poll_external_updates().await;
            all_notices.extend(update.notices);
            if update.pending.is_empty() {
                return Ok(all_notices);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(McpNotReady {
                    pending: update.pending,
                    notices: all_notices,
                    waited: started.elapsed(),
                });
            }
            match next {
                Some(wait) => wait.wait(Some(deadline)).await,
                None => tokio::time::sleep_until(deadline).await,
            }
        }
    }

    /// Stage a reload for every active MCP server under one router lock and
    /// return the typed per-server report (K14).
    ///
    /// This is the lifecycle owner's reload-all primitive: surfaces (RPC,
    /// REST, MCP server) consume the [`McpReloadReport`] and project a
    /// non-clean report as a typed wire fault, instead of hand-rolling
    /// per-server stage loops that abort halfway with partial staging.
    pub async fn stage_reload_all(&self) -> Result<McpReloadReport, McpError> {
        if self.is_closing() {
            return Err(McpError::RouterShutDown);
        }
        let mut router = self.router.write().await;
        let router = router.as_mut().ok_or(McpError::RouterShutDown)?;
        let mut report = McpReloadReport::default();
        for name in router.active_server_names() {
            match router.stage_reload(McpReloadTarget::ServerName(name.clone())) {
                Ok(()) => report.staged.push(name),
                Err(error) => report.failed.push(McpReloadFailure {
                    server: name,
                    error,
                }),
            }
        }
        self.sync_router_projection(router);
        Ok(report)
    }

    /// Test helper for cross-crate lifecycle integration tests.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn set_inflight_calls_for_testing(
        &self,
        server_name: &str,
        count: usize,
    ) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .set_inflight_calls_for_testing(server_name, count)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    /// Test helper for controlling removal timeout from integration tests.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn set_removal_timeout_for_testing(
        &self,
        removal_timeout: Duration,
    ) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router
            .set_removal_timeout(removal_timeout)
            .map_err(|error| error.to_string())?;
        self.sync_router_projection(router);
        Ok(())
    }

    fn has_pre_bind_surface_facts(snapshot: &ExternalToolSurfaceSnapshot) -> bool {
        snapshot.phase != meerkat_core::ExternalToolSurfaceGlobalPhase::Operating
            || snapshot.snapshot_epoch != 0
            || snapshot.snapshot_aligned_epoch != 0
            || !snapshot.entries.is_empty()
    }

    fn seed_now_ms() -> u64 {
        meerkat_core::time_compat::SystemTime::now()
            .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }

    /// Re-derive pre-bind surface facts through generated inputs on the
    /// newly bound session authority (the same seed pattern as
    /// `bind_mcp_server_lifecycle_handle`'s `apply_connect_pending` loop).
    ///
    /// The pre-bind state was built exclusively through the SAME generated
    /// inputs on the router's construction-time authority (typically
    /// `RuntimeExternalToolSurfaceHandle::ephemeral()`), so it is
    /// machine-validated state on the wrong authority instance — not a
    /// handwritten snapshot. Every fact re-enters the session authority
    /// through its own typed inputs and the machine re-validates each
    /// transition; a refusal fails the seed and the caller poisons
    /// fail-closed. Without this, the common embedder flow (stage/connect
    /// tool servers BEFORE the agent build delivers the session bind)
    /// poisoned the surface permanently and every external tool call was
    /// rejected thereafter.
    /// Stage `input` on `new` and return the exact lineage it recorded.
    fn stage_recording_lineage(
        new: &dyn ExternalToolSurfaceHandle,
        input: meerkat_core::handles::ExternalToolSurfaceInput,
    ) -> Result<u64, String> {
        let transition = new
            .apply_surface_input(input)
            .map_err(|error| error.to_string())?;
        transition
            .effects
            .iter()
            .find_map(|effect| match effect {
                meerkat_core::handles::ExternalToolSurfaceEffect::StageRecorded {
                    staged_intent_sequence,
                    ..
                } => Some(*staged_intent_sequence),
                _ => None,
            })
            .ok_or_else(|| "session authority recorded no staged lineage".to_string())
    }

    /// Re-derive the pre-bind surface facts on `new` and return the lineages
    /// the re-derivation staged there, which the router now holds.
    fn seed_pre_bind_surface_facts(
        snapshot: &ExternalToolSurfaceSnapshot,
        new: &dyn ExternalToolSurfaceHandle,
    ) -> Result<std::collections::BTreeMap<String, BTreeSet<u64>>, String> {
        use meerkat_core::handles::ExternalToolSurfaceInput as SeedInput;
        let mut held: std::collections::BTreeMap<String, BTreeSet<u64>> =
            std::collections::BTreeMap::new();
        use meerkat_core::{
            ExternalToolSurfaceBaseState as Base, ExternalToolSurfacePendingOp as Pending,
            ExternalToolSurfaceStagedOp as Staged,
        };

        let now_ms = Self::seed_now_ms();
        for entry in &snapshot.entries {
            let surface_id = entry.surface_id.clone();
            let seed_err = |stage: &str, error: &dyn std::fmt::Display| {
                format!(
                    "session authority refused pre-bind surface re-derivation \
                     ({stage} for '{surface_id}'): {error}"
                )
            };
            if entry.base_state == Base::Removed {
                // A removed surface carries no live obligations; nothing to
                // re-derive on the session authority.
                continue;
            }
            new.register(surface_id.clone())
                .map_err(|error| seed_err("register", &error))?;

            // Everything except a never-applied staged Add reached its state
            // through an applied Add: re-derive the Add lifecycle first.
            let needs_applied_add =
                entry.base_state != Base::Absent || entry.pending_op == Pending::Add;
            let staged_add_only = entry.base_state == Base::Absent
                && entry.pending_op == Pending::None
                && entry.staged_op == Staged::Add;

            let mut added_seq = None;
            if needs_applied_add || staged_add_only {
                let seq = Self::stage_recording_lineage(
                    new,
                    SeedInput::StageAdd {
                        surface_id: surface_id.clone(),
                        now_ms,
                    },
                )
                .map_err(|error| seed_err("stage_add", &error))?;
                held.entry(surface_id.clone()).or_default().insert(seq);
                added_seq = Some(seq);
            }
            let mut applied_staged_seq = None;
            if needs_applied_add {
                // The stage's own recorded lineage: completion inputs carry
                // the sequence pair from the boundary application, exactly
                // like the router's ScheduleSurfaceCompletion effects do.
                let staged_seq = added_seq.ok_or_else(|| {
                    seed_err(
                        "stage_add",
                        &"session authority recorded no staged sequence",
                    )
                })?;
                new.apply_boundary(surface_id.clone(), now_ms, staged_seq, 0)
                    .map_err(|error| seed_err("apply_boundary", &error))?;
                applied_staged_seq = Some(staged_seq);
            }
            if entry.base_state != Base::Absent {
                // The surface was Active (or draining): its pending Add
                // completed on the old authority; complete it here too.
                let pending_seq = new
                    .surface_snapshot(&surface_id)
                    .and_then(|seeded| seeded.pending_task_sequence)
                    .ok_or_else(|| {
                        seed_err(
                            "apply_boundary",
                            &"session authority recorded no pending sequence",
                        )
                    })?;
                let staged_seq = applied_staged_seq.ok_or_else(|| {
                    seed_err(
                        "apply_boundary",
                        &"seed applied no boundary for an active surface",
                    )
                })?;
                new.mark_pending_succeeded(surface_id.clone(), pending_seq, staged_seq)
                    .map_err(|error| seed_err("mark_pending_succeeded", &error))?;
            }

            // Re-derive any live follow-up intent on the Active base.
            let follow_up = match entry.staged_op {
                Staged::Remove => Some((
                    "stage_remove",
                    SeedInput::StageRemove {
                        surface_id: surface_id.clone(),
                        now_ms,
                    },
                )),
                Staged::Reload => Some((
                    "stage_reload",
                    SeedInput::StageReload {
                        surface_id: surface_id.clone(),
                        now_ms,
                    },
                )),
                Staged::Add | Staged::None => None,
            };
            let mut follow_up_seq = None;
            if let Some((stage, input)) = follow_up {
                let seq = Self::stage_recording_lineage(new, input)
                    .map_err(|error| seed_err(stage, &error))?;
                held.entry(surface_id.clone()).or_default().insert(seq);
                follow_up_seq = Some(seq);
            }
            if entry.base_state == Base::Removing {
                let staged_seq = follow_up_seq.ok_or_else(|| {
                    seed_err(
                        "stage_remove",
                        &"session authority recorded no removal sequence",
                    )
                })?;
                new.apply_boundary(surface_id.clone(), now_ms, staged_seq, 0)
                    .map_err(|error| seed_err("apply_boundary(remove)", &error))?;
            }
            for _ in 0..entry.inflight_call_count {
                new.call_started(surface_id.clone())
                    .map_err(|error| seed_err("call_started", &error))?;
            }
        }

        // Mirror the old authority's publication alignment so the seeded
        // visible set publishes the same way it had pre-bind.
        if snapshot.snapshot_aligned_epoch == snapshot.snapshot_epoch {
            let new_epoch = new.snapshot_epoch();
            if new_epoch != new.snapshot_aligned_epoch() {
                new.snapshot_aligned(new_epoch).map_err(|error| {
                    format!(
                        "session authority refused pre-bind surface re-derivation \
                         (snapshot_aligned): {error}"
                    )
                })?;
            }
        }
        Ok(held)
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouterAdapter {
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        let guard = self.router.read().await;
        match guard.as_ref() {
            Some(router) => {
                router
                    .resolve_tool_application(source_tool, request, invocation, context)
                    .await
            }
            None => Err(ToolError::access_denied(source_tool)),
        }
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self.router.try_read() {
            Ok(router) => match router.as_ref() {
                Some(router) => {
                    let tools = AgentToolDispatcher::tools(router);
                    self.set_tools_cache(tools.clone());
                    self.set_catalog_cache(AgentToolDispatcher::tool_catalog(router));
                    tools
                }
                None => self.cached_tools(),
            },
            Err(_) => self.cached_tools(),
        }
    }

    fn external_tool_surface_snapshot(&self) -> Option<ExternalToolSurfaceSnapshot> {
        match self.router.try_read() {
            Ok(router) => match router.as_ref() {
                Some(router) => {
                    let snapshot = McpRouter::external_tool_surface_snapshot(router);
                    self.set_surface_snapshot_cache(Some(snapshot.clone()));
                    Some(snapshot)
                }
                None => self.cached_surface_snapshot(),
            },
            Err(_) => self.cached_surface_snapshot(),
        }
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        self.router
            .try_read()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|router| AgentToolDispatcher::tool_mutation_class(router, tool_name))
            })
            .unwrap_or(meerkat_core::ToolMutationClass::Unknown)
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.router
            .try_read()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|router| AgentToolDispatcher::review_entry_support(router, tool_name))
            })
            .unwrap_or_default()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if self.is_closing() {
            return Err(ToolError::execution_failed("MCP router is closing"));
        }
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => router.dispatch_with_context(call, context).await,
            None => Err(ToolError::execution_failed("MCP router has been shut down")),
        }
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        if self.is_closing() {
            return Err(ToolError::execution_failed("MCP router is closing"));
        }
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => {
                if !self.binding_witness_matches(
                    call.name,
                    self.live_binding_fingerprint(router, call.name),
                    plan,
                ) || !self.operation_target_matches(router, call.name, plan)
                {
                    return Err(ToolError::unavailable(
                        call.name,
                        meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                    ));
                }
                AgentToolDispatcher::dispatch_resolved_with_context(router, call, context, plan)
                    .await
            }
            None => Err(ToolError::unavailable(
                call.name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            )),
        }
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        // Fast path: skip write lock if nothing is pending.
        if !self.has_pending.load(Ordering::Acquire) {
            return ExternalToolUpdate::default();
        }

        let mut router = self.router.write().await;
        let Some(router) = router.as_mut() else {
            return ExternalToolUpdate::default();
        };

        let update = router.take_external_updates();
        if !update.notices.is_empty() {
            self.execution_binding_epoch.fetch_add(1, Ordering::AcqRel);
        }

        self.sync_router_projection(router);
        update
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }

    fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
        self.execution_binding_epoch.load(Ordering::Acquire)
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        match self.router.try_read() {
            Ok(router) => match router.as_ref() {
                Some(router) => {
                    let catalog = AgentToolDispatcher::tool_catalog(router);
                    self.set_catalog_cache(catalog.clone());
                    catalog
                }
                None => self.cached_catalog(),
            },
            Err(_) => self.cached_catalog(),
        }
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        match self.router.try_read() {
            Ok(router) => match router.as_ref() {
                Some(router) => AgentToolDispatcher::resolve_execution_plan(
                    router,
                    call,
                    dispatch_context,
                    resolution_context,
                )?
                .with_owner_witness(self.owner_witness(router, call.name)?),
                None => Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                    tool_name: call.name.to_string(),
                    reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                }),
            },
            Err(_) => Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            }),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.cached_pending_sources()
    }

    fn bind_mcp_server_lifecycle_handle(&self, handle: Arc<dyn McpServerLifecycleHandle>) {
        // Seed the session DSL with any servers that were staged / are
        // currently connecting from *before* the handle was bound (router is
        // typically constructed and stage_add'd prior to the agent build that
        // owns the runtime bindings). Without this seed, `pending_server_ids`
        // from the DSL would return empty even while the shell router has
        // real background connections in flight — the `[MCP_PENDING]` notice
        // would then go silent immediately after bind.
        //
        // K14: a rejected seed apply means the session DSL refused to mirror
        // real router state — fail closed by binding a poisoned handle that
        // (a) keeps the seeded servers visibly pending so the `[MCP_PENDING]`
        // notice never goes silently quiet, and (b) returns the typed seed
        // rejection for every subsequent lifecycle apply, which propagates
        // through the router's fail-closed notify paths.
        let pending_before_bind: Vec<String> =
            self.cached_pending_sources().iter().cloned().collect();
        let mut seed_fault = None;
        for server_name in &pending_before_bind {
            if let Err(error) = handle.apply_connect_pending(server_name) {
                seed_fault = Some(error);
                break;
            }
        }
        let handle = match seed_fault {
            None => handle,
            Some(error) => {
                tracing::warn!(
                    error = %error,
                    "seed apply_connect_pending on bind rejected by DSL; poisoning MCP lifecycle mirror"
                );
                Arc::new(PoisonedMcpServerLifecycleHandle {
                    reason: error,
                    pending: pending_before_bind.into_iter().collect(),
                }) as Arc<dyn McpServerLifecycleHandle>
            }
        };

        // Write into the shared slot cloned from the inner router; no async
        // lock is acquired. Subsequent router handshake events flow through
        // this handle into the session's MeerkatMachine DSL.
        let mut slot = match self.mcp_lifecycle_handle.write() {
            Ok(slot) => slot,
            Err(poisoned) => {
                tracing::warn!("McpRouterAdapter mcp_lifecycle_handle RwLock poisoned; recovering");
                poisoned.into_inner()
            }
        };
        *slot = Some(handle);
    }

    fn bind_external_tool_surface_handle(&self, handle: Arc<dyn ExternalToolSurfaceHandle>) {
        let Some(slot) = &self.external_surface_handle else {
            return;
        };
        #[cfg(test)]
        if let Some(attempt) = self
            .bind_claim_attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = attempt.send(());
        }
        let current = match slot.read() {
            Ok(slot) => Arc::clone(&*slot),
            Err(poisoned) => {
                tracing::warn!(
                    "McpRouterAdapter external_surface_handle RwLock poisoned during bind read; recovering"
                );
                let slot = poisoned.into_inner();
                Arc::clone(&*slot)
            }
        };
        if Arc::ptr_eq(&current, &handle) {
            return;
        }
        let mut poisoned_handle = None;
        // Lineages staged on the old surface mean nothing on the new one;
        // the router holds only what the re-derivation stages there.
        let mut rebound_lineages = std::collections::BTreeMap::new();
        // Live snapshot: the cached copy can predate connection completions
        // that already landed on the pre-bind authority.
        let pre_bind_snapshot = self
            .external_tool_surface_snapshot()
            .or_else(|| self.cached_surface_snapshot());
        if let Some(snapshot) = pre_bind_snapshot
            && Self::has_pre_bind_surface_facts(&snapshot)
        {
            // Pre-bind facts (servers staged/connected before the session
            // authority arrived — the normal embedder spawn flow) are
            // re-derived through generated inputs on the new authority so
            // the session machine re-validates every fact. Only a machine
            // REFUSAL poisons fail-closed.
            match Self::seed_pre_bind_surface_facts(&snapshot, handle.as_ref()) {
                Ok(held) => {
                    rebound_lineages = held;
                    tracing::info!(
                        surfaces = snapshot.entries.len(),
                        "re-derived pre-bind external tool surface facts onto session authority"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "session authority refused pre-bind surface re-derivation; poisoning surface owner"
                    );
                    poisoned_handle = Some(Arc::new(PoisonedExternalToolSurfaceHandle::new(
                        error, snapshot,
                    ))
                        as Arc<dyn ExternalToolSurfaceHandle>);
                }
            }
        }
        let mut guard = match slot.write() {
            Ok(slot) => slot,
            Err(poisoned) => {
                tracing::warn!(
                    "McpRouterAdapter external_surface_handle RwLock poisoned; recovering"
                );
                poisoned.into_inner()
            }
        };
        // The handle, its ownership and the held lineages change together
        // under the slot's write lock; the router stages (recording a
        // lineage) under a read hold of it and ends its hold under one
        // exclusive claim of it. The bound surface belongs to the binder
        // (the session registration), not to this router: shutting the
        // router down must release only its own lineages there, never shut
        // that surface down.
        let published = self
            .surface_ownership
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .rebind_borrowed();
        if !published {
            // The router already ended its hold: publish nothing to it, and
            // release what the re-derivation staged on the new surface.
            drop(guard);
            crate::router::release_lineages(handle.as_ref(), rebound_lineages);
            return;
        }
        *self
            .held_surface_lineages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = rebound_lineages;
        *guard = poisoned_handle.unwrap_or(handle);
        drop(guard);
    }
}

/// The agent that owns this adapter's router releases it when it ends: a
/// borrowed surface has the router's own lineages released, an owned one is
/// shut down.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::AgentOwnedRelease for McpRouterAdapter {
    async fn release(&self) {
        self.shutdown().await;
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::McpOperationObserver for McpRouterAdapter {
    async fn observe_operation(
        &self,
        server: &str,
        raw_operation: &str,
    ) -> meerkat_core::McpOperationObservation {
        McpRouterAdapter::observe_operation(self, server, raw_operation).await
    }
}

#[cfg(test)]
#[path = "shutdown_observer_tests.rs"]
mod shutdown_observer_tests;

#[cfg(all(test, unix, feature = "workspace-tests"))]
#[path = "stdio_diagnostics_tests.rs"]
mod stdio_diagnostics_tests;

#[cfg(all(test, unix))]
#[path = "attempt_guard_tests.rs"]
mod attempt_guard_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::McpRouter;
    use crate::connection::McpConnection;
    use meerkat_core::ExternalToolSurfacePendingOp;
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    #[cfg(feature = "workspace-tests")]
    use meerkat_runtime::RuntimeExternalToolSurfaceHandle;
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;

    fn async_connect_test_timeout() -> Duration {
        Duration::from_secs((McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS as u64) + 5)
    }

    fn test_server_config(name: &str, path: &Path) -> meerkat_core::McpServerConfig {
        meerkat_core::McpServerConfig::stdio(
            name,
            path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        )
    }

    #[cfg(feature = "workspace-tests")]
    fn generated_surface_handle() -> Arc<dyn ExternalToolSurfaceHandle> {
        Arc::new(RuntimeExternalToolSurfaceHandle::ephemeral())
    }

    #[cfg(feature = "workspace-tests")]
    fn generated_surface_router() -> McpRouter {
        McpRouter::new_with_surface_handle_and_stdio_profile(
            generated_surface_handle(),
            crate::McpStdioLaunchProfile::trusted_host(),
        )
    }

    struct RejectingExternalToolSurfaceHandle;

    impl RejectingExternalToolSurfaceHandle {
        fn reject(context: &'static str) -> DslTransitionError {
            DslTransitionError::guard_rejected(context, "injected bind failure")
        }
    }

    impl ExternalToolSurfaceHandle for RejectingExternalToolSurfaceHandle {
        fn apply_surface_input(
            &self,
            _input: ExternalToolSurfaceInput,
        ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::apply_surface_input",
            ))
        }

        fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject("RejectingExternalToolSurfaceHandle::register"))
        }

        fn stage_add(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::stage_add",
            ))
        }

        fn stage_remove(
            &self,
            _surface_id: String,
            _now_ms: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::stage_remove",
            ))
        }

        fn stage_reload(
            &self,
            _surface_id: String,
            _now_ms: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::stage_reload",
            ))
        }

        fn apply_boundary(
            &self,
            _surface_id: String,
            _now_ms: u64,
            _staged_intent_sequence: u64,
            _applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::apply_boundary",
            ))
        }

        fn mark_pending_succeeded(
            &self,
            _surface_id: String,
            _pending_task_sequence: u64,
            _staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::mark_pending_succeeded",
            ))
        }

        fn mark_pending_failed(
            &self,
            _surface_id: String,
            _pending_task_sequence: u64,
            _staged_intent_sequence: u64,
            _cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::mark_pending_failed",
            ))
        }

        fn call_started(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::call_started",
            ))
        }

        fn call_finished(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::call_finished",
            ))
        }

        fn finalize_removal_clean(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::finalize_removal_clean",
            ))
        }

        fn finalize_removal_forced(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::finalize_removal_forced",
            ))
        }

        fn snapshot_aligned(&self, _epoch: u64) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::snapshot_aligned",
            ))
        }

        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            Err(Self::reject(
                "RejectingExternalToolSurfaceHandle::shutdown_surface",
            ))
        }

        fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
            None
        }

        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            SurfaceDiagnosticSnapshot {
                surface_phase: meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
                known_surfaces: BTreeSet::new(),
                visible_surfaces: BTreeSet::new(),
                snapshot_epoch: 0,
                snapshot_aligned_epoch: 0,
                has_pending_or_staged: false,
                entries: Vec::new(),
            }
        }

        fn visible_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn removing_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn pending_surfaces(&self) -> BTreeSet<String> {
            BTreeSet::new()
        }

        fn has_pending_or_staged(&self) -> bool {
            false
        }

        fn snapshot_epoch(&self) -> u64 {
            0
        }

        fn snapshot_aligned_epoch(&self) -> u64 {
            0
        }
    }

    /// A prepared attachment control whose apply decision the test sets.
    struct TestAttachment {
        server: meerkat_core::McpServerConfig,
        apply: Result<(), McpAttachmentRefusal>,
        apply_checks: std::sync::atomic::AtomicUsize,
    }

    impl McpAttachmentAuthority for TestAttachment {
        fn server(&self) -> &meerkat_core::McpServerConfig {
            &self.server
        }
        fn authorize_apply(&self) -> Result<(), McpAttachmentRefusal> {
            self.apply_checks.fetch_add(1, Ordering::SeqCst);
            self.apply.clone()
        }
        fn authorize_persist(
            &self,
            _scope: meerkat_core::mcp_config::McpScope,
        ) -> Result<(), McpAttachmentRefusal> {
            Err(McpAttachmentRefusal::Denied)
        }
    }

    fn test_attachment(
        server: &meerkat_core::McpServerConfig,
        apply: Result<(), McpAttachmentRefusal>,
    ) -> Arc<TestAttachment> {
        Arc::new(TestAttachment {
            server: server.clone(),
            apply,
            apply_checks: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn marker_server(name: &str, marker: &Path) -> meerkat_core::McpServerConfig {
        meerkat_core::McpServerConfig::stdio(
            name,
            "/bin/sh".to_string(),
            vec!["-c".to_string(), format!("touch '{}'", marker.display())],
            HashMap::new(),
        )
    }

    /// Wait until the background connection has asked `control` at its
    /// physical entry, then let a refused attempt settle.
    async fn until_entry_asked(control: &TestAttachment) {
        tokio::time::timeout(async_connect_test_timeout(), async {
            while control.apply_checks.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the connection reaches its physical entry");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn governed_control_is_staged_with_its_payload_and_asked_only_at_physical_entry() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("spawned");
        let adapter = McpRouterAdapter::new(generated_surface_router());
        let server = marker_server("refused", &marker);
        let control = test_attachment(&server, Err(McpAttachmentRefusal::Denied));
        adapter
            .stage_add_governed(server, control.clone())
            .await
            .expect("stage under a prepared control");
        // Staging and enqueueing ask nothing; the control travels with the
        // exact payload.
        assert_eq!(control.apply_checks.load(Ordering::SeqCst), 0);
        adapter
            .apply_staged()
            .await
            .expect("apply boundary enqueues");
        until_entry_asked(&control).await;
        assert_eq!(control.apply_checks.load(Ordering::SeqCst), 1);
        assert!(!marker.exists(), "a refused entry never spawns the server");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn an_allowed_entry_is_asked_exactly_once() {
        let adapter = McpRouterAdapter::new(generated_surface_router());
        let server = test_server_config("allowed", Path::new("/nonexistent/mcp-server"));
        let control = test_attachment(&server, Ok(()));
        adapter
            .stage_add_governed(server, control.clone())
            .await
            .expect("stage under a prepared control");
        adapter
            .apply_staged()
            .await
            .expect("apply boundary enqueues");
        until_entry_asked(&control).await;
        assert_eq!(control.apply_checks.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_same_name_replacement_never_applies_under_the_earlier_control() {
        let dir = tempfile::tempdir().unwrap();
        let marker_a = dir.path().join("a");
        let marker_b = dir.path().join("b");
        let adapter = McpRouterAdapter::new(generated_surface_router());
        let server_a = marker_server("same", &marker_a);
        let server_b = marker_server("same", &marker_b);
        let control_a = test_attachment(&server_a, Ok(()));
        let control_b = test_attachment(&server_b, Err(McpAttachmentRefusal::Denied));
        adapter
            .stage_add_governed(server_a, control_a.clone())
            .await
            .expect("stage A");
        adapter
            .stage_add_governed(server_b, control_b.clone())
            .await
            .expect("stage B replaces A with its own control");
        adapter
            .apply_staged()
            .await
            .expect("apply boundary enqueues");
        until_entry_asked(&control_b).await;
        assert_eq!(control_a.apply_checks.load(Ordering::SeqCst), 0);
        assert_eq!(control_b.apply_checks.load(Ordering::SeqCst), 1);
        assert!(!marker_a.exists() && !marker_b.exists());
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_governed_name_never_changes_without_a_control() {
        let adapter = McpRouterAdapter::new(generated_surface_router());
        let server = test_server_config("governed", Path::new("/nonexistent/mcp-server"));
        let other = test_server_config("governed", Path::new("/nonexistent/other-server"));
        adapter
            .stage_add_governed(server.clone(), test_attachment(&server, Ok(())))
            .await
            .expect("stage under a prepared control");
        let plain = adapter.stage_add(other.clone()).await;
        assert!(
            plain.is_err(),
            "a plain add cannot replace a governed payload"
        );
        let reload = adapter
            .stage_reload(crate::McpReloadTarget::Config(other))
            .await;
        assert!(
            reload.is_err(),
            "a reload cannot replace a governed payload"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn governed_add_refuses_a_control_for_another_descriptor() {
        let adapter = McpRouterAdapter::new(generated_surface_router());
        let server = test_server_config("one", Path::new("/nonexistent/mcp-server"));
        let other = test_server_config("one", Path::new("/nonexistent/other-server"));
        let control = test_attachment(&other, Ok(()));
        let error = adapter
            .stage_add_governed(server, control.clone())
            .await
            .expect_err("a control covers only its exact descriptor");
        assert!(
            error.contains(&McpAttachmentRefusal::Mismatch.to_string()),
            "{error}"
        );
        let result = adapter.apply_staged().await.expect("apply boundary");
        assert!(result.delta.lifecycle_actions.is_empty());
        assert_eq!(control.apply_checks.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_staged_removal_keeps_a_withdrawn_governed_add_governed_until_its_boundary() {
        let mut router = generated_surface_router();
        let server = test_server_config("governed", Path::new("/nonexistent/mcp-server"));
        let other = test_server_config("governed", Path::new("/nonexistent/other-server"));
        router
            .stage_add_governed(server.clone(), test_attachment(&server, Ok(())))
            .expect("stage under a prepared control");
        router.stage_remove("governed").expect("stage the removal");
        // The removal is intent only: a plain add cannot replace it before
        // its boundary.
        assert!(matches!(
            router.stage_add(other.clone()),
            Err(McpError::AttachmentRefused(McpAttachmentRefusal::Mismatch))
        ));
        router.apply_staged().await.expect("apply boundary");
        // The withdrawn add never attached: after the boundary the name is
        // available to an ungoverned add.
        router
            .stage_add(other)
            .expect("an ungoverned add after the removal's disposition");
        router.shutdown().await;
    }

    /// The real surface handle, except that the next removal boundary first
    /// meets another holder's newer staged removal of the same surface, so the
    /// router's captured sequence is stale and the boundary is rejected.
    struct StaleRemovalBoundary {
        inner: Arc<dyn ExternalToolSurfaceHandle>,
        armed: AtomicBool,
    }

    impl ExternalToolSurfaceHandle for StaleRemovalBoundary {
        fn apply_surface_input(
            &self,
            input: ExternalToolSurfaceInput,
        ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
            if let ExternalToolSurfaceInput::ApplyBoundary { surface_id, .. } = &input
                && self.armed.swap(false, Ordering::SeqCst)
            {
                self.inner.stage_remove(surface_id.clone(), 0)?;
            }
            self.inner.apply_surface_input(input)
        }
        fn register(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.register(surface_id)
        }
        fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_add(surface_id, now_ms)
        }
        fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_remove(surface_id, now_ms)
        }
        fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_reload(surface_id, now_ms)
        }
        fn apply_boundary(
            &self,
            surface_id: String,
            now_ms: u64,
            staged_intent_sequence: u64,
            applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner
                .apply_boundary(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
        }
        fn mark_pending_succeeded(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_succeeded(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            )
        }
        fn mark_pending_failed(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
            cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_failed(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            )
        }
        fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_started(surface_id)
        }
        fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_finished(surface_id)
        }
        fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.finalize_removal_clean(surface_id)
        }
        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.finalize_removal_forced(surface_id)
        }
        fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
            self.inner.snapshot_aligned(epoch)
        }
        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            self.inner.shutdown_surface()
        }
        fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
            self.inner.surface_snapshot(surface_id)
        }
        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            self.inner.diagnostic_snapshot()
        }
        fn visible_surfaces(&self) -> BTreeSet<String> {
            self.inner.visible_surfaces()
        }
        fn removing_surfaces(&self) -> BTreeSet<String> {
            self.inner.removing_surfaces()
        }
        fn pending_surfaces(&self) -> BTreeSet<String> {
            self.inner.pending_surfaces()
        }
        fn has_pending_or_staged(&self) -> bool {
            self.inner.has_pending_or_staged()
        }
        fn snapshot_epoch(&self) -> u64 {
            self.inner.snapshot_epoch()
        }
        fn snapshot_aligned_epoch(&self) -> u64 {
            self.inner.snapshot_aligned_epoch()
        }
    }

    /// The real surface handle, except that the first input `gate_on`
    /// matches reports itself entered and then waits to be let through.
    struct GatedSurface {
        inner: Arc<dyn ExternalToolSurfaceHandle>,
        gate_on: fn(&ExternalToolSurfaceInput) -> bool,
        entered: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
        go: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    #[cfg(feature = "workspace-tests")]
    impl GatedSurface {
        fn new(
            gate_on: fn(&ExternalToolSurfaceInput) -> bool,
        ) -> (
            Arc<Self>,
            std::sync::mpsc::Receiver<()>,
            std::sync::mpsc::Sender<()>,
        ) {
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let (go_tx, go_rx) = std::sync::mpsc::channel();
            let gated = Arc::new(Self {
                inner: generated_surface_handle(),
                gate_on,
                entered: std::sync::Mutex::new(Some(entered_tx)),
                go: std::sync::Mutex::new(go_rx),
            });
            (gated, entered_rx, go_tx)
        }
    }

    impl ExternalToolSurfaceHandle for GatedSurface {
        fn apply_surface_input(
            &self,
            input: ExternalToolSurfaceInput,
        ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
            if (self.gate_on)(&input)
                && let Some(entered) = self.entered.lock().unwrap().take()
            {
                entered.send(()).unwrap();
                self.go.lock().unwrap().recv().unwrap();
            }
            self.inner.apply_surface_input(input)
        }
        fn register(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.register(surface_id)
        }
        fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_add(surface_id, now_ms)
        }
        fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_remove(surface_id, now_ms)
        }
        fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.inner.stage_reload(surface_id, now_ms)
        }
        fn apply_boundary(
            &self,
            surface_id: String,
            now_ms: u64,
            staged_intent_sequence: u64,
            applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner
                .apply_boundary(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
        }
        fn mark_pending_succeeded(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_succeeded(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            )
        }
        fn mark_pending_failed(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
            cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            self.inner.mark_pending_failed(
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            )
        }
        fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_started(surface_id)
        }
        fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.call_finished(surface_id)
        }
        fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.finalize_removal_clean(surface_id)
        }
        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.inner.finalize_removal_forced(surface_id)
        }
        fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
            self.inner.snapshot_aligned(epoch)
        }
        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            self.inner.shutdown_surface()
        }
        fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
            self.inner.surface_snapshot(surface_id)
        }
        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            self.inner.diagnostic_snapshot()
        }
        fn visible_surfaces(&self) -> BTreeSet<String> {
            self.inner.visible_surfaces()
        }
        fn removing_surfaces(&self) -> BTreeSet<String> {
            self.inner.removing_surfaces()
        }
        fn pending_surfaces(&self) -> BTreeSet<String> {
            self.inner.pending_surfaces()
        }
        fn has_pending_or_staged(&self) -> bool {
            self.inner.has_pending_or_staged()
        }
        fn snapshot_epoch(&self) -> u64 {
            self.inner.snapshot_epoch()
        }
        fn snapshot_aligned_epoch(&self) -> u64 {
            self.inner.snapshot_aligned_epoch()
        }
    }

    /// A bind racing an owned router's shutdown waits for the shutdown's
    /// claim: the shutdown reaches only the router's own private surface,
    /// never the registration surface the bind publishes.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bind_racing_an_owned_shutdown_waits_for_its_claim() {
        let (private, entered, go) =
            GatedSurface::new(|input| matches!(input, ExternalToolSurfaceInput::Shutdown));
        let adapter = Arc::new(McpRouterAdapter::new(
            McpRouter::new_with_surface_handle_and_stdio_profile(
                private.clone(),
                crate::McpStdioLaunchProfile::trusted_host(),
            )
            .with_surface_ownership(crate::McpSurfaceOwnership::Owned),
        ));
        let shutting = tokio::spawn({
            let adapter = Arc::clone(&adapter);
            async move { adapter.shutdown().await }
        });
        // The shutdown is inside its claim, applying Shutdown to the
        // private surface.
        tokio::task::spawn_blocking(move || entered.recv())
            .await
            .unwrap()
            .expect("the shutdown reached its surface");
        let registration = generated_surface_handle();
        let (attempted, bind_attempt) = std::sync::mpsc::channel();
        *adapter.bind_claim_attempt.lock().unwrap() = Some(attempted);
        let binding = std::thread::spawn({
            let adapter = Arc::clone(&adapter);
            let registration = Arc::clone(&registration);
            move || adapter.bind_external_tool_surface_handle(registration)
        });
        // The acknowledgement is sent just before the bind's first read of
        // the slot, so the bind thread has started and reached that point;
        // it removes the "not yet started" ambiguity, not all scheduling
        // ambiguity. The gated claim still holds the slot, so the bind
        // cannot have read or published anything while it is held.
        bind_attempt
            .recv_timeout(Duration::from_secs(10))
            .expect("the bind reached its claim attempt");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!binding.is_finished(), "the bind waits for the claim");
        go.send(()).unwrap();
        shutting.await.unwrap();
        binding.join().unwrap();
        assert_eq!(
            private.diagnostic_snapshot().surface_phase,
            meerkat_core::ExternalToolSurfaceGlobalPhase::Shutdown
        );
        assert_eq!(
            registration.diagnostic_snapshot().surface_phase,
            meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
            "the owned shutdown never reaches the rebound surface"
        );
    }

    /// A bind racing a borrowed router's lineage release waits for the
    /// release's claim: the release consumes exactly the lineages the router
    /// held on the surface it claimed, and the rebound surface is untouched.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bind_racing_a_borrowed_release_waits_for_its_claim() {
        let (claimed, entered, go) =
            GatedSurface::new(|input| matches!(input, ExternalToolSurfaceInput::Release { .. }));
        let adapter = Arc::new(McpRouterAdapter::new(
            McpRouter::new_with_surface_handle_and_stdio_profile(
                claimed.clone(),
                crate::McpStdioLaunchProfile::trusted_host(),
            ),
        ));
        adapter
            .stage_add(test_server_config(
                "held",
                Path::new("/nonexistent/mcp-server"),
            ))
            .await
            .expect("a lineage the router holds");
        let releasing = tokio::spawn({
            let adapter = Arc::clone(&adapter);
            async move { adapter.shutdown().await }
        });
        tokio::task::spawn_blocking(move || entered.recv())
            .await
            .unwrap()
            .expect("the release reached its claimed surface");
        let rebound = generated_surface_handle();
        let (attempted, bind_attempt) = std::sync::mpsc::channel();
        *adapter.bind_claim_attempt.lock().unwrap() = Some(attempted);
        let binding = std::thread::spawn({
            let adapter = Arc::clone(&adapter);
            let rebound = Arc::clone(&rebound);
            move || adapter.bind_external_tool_surface_handle(rebound)
        });
        // The acknowledgement is sent just before the bind's first read of
        // the slot, so the bind thread has started and reached that point;
        // it removes the "not yet started" ambiguity, not all scheduling
        // ambiguity. The gated claim still holds the slot, so the bind
        // cannot have read or published anything while it is held.
        bind_attempt
            .recv_timeout(Duration::from_secs(10))
            .expect("the bind reached its claim attempt");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!binding.is_finished(), "the bind waits for the claim");
        go.send(()).unwrap();
        releasing.await.unwrap();
        binding.join().unwrap();
        assert_eq!(
            claimed
                .surface_snapshot("held")
                .map(|entry| entry.staged_op),
            Some(ExternalToolSurfaceStagedOp::None),
            "the claimed surface's own lineage is released"
        );
        assert_eq!(
            claimed.diagnostic_snapshot().surface_phase,
            meerkat_core::ExternalToolSurfaceGlobalPhase::Operating,
            "a borrowed surface is never shut down"
        );
        assert!(
            rebound
                .surface_snapshot("held")
                .is_none_or(|entry| entry.staged_op == ExternalToolSurfaceStagedOp::None),
            "the rebound surface keeps nothing of the ended router"
        );
    }

    /// A bind that began re-deriving its pre-bind facts before the router
    /// ended its hold publishes nothing afterwards and releases what it
    /// staged on the registration surface.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bind_finishing_after_the_router_ended_publishes_nothing() {
        let adapter = Arc::new(McpRouterAdapter::new(
            McpRouter::new_with_surface_handle_and_stdio_profile(
                generated_surface_handle(),
                crate::McpStdioLaunchProfile::trusted_host(),
            )
            .with_surface_ownership(crate::McpSurfaceOwnership::Owned),
        ));
        adapter
            .stage_add(test_server_config(
                "raced",
                Path::new("/nonexistent/mcp-server"),
            ))
            .await
            .expect("a pre-bind staged fact");
        let (registration, entered, go) =
            GatedSurface::new(|input| matches!(input, ExternalToolSurfaceInput::StageAdd { .. }));
        let binding = std::thread::spawn({
            let adapter = Arc::clone(&adapter);
            let registration: Arc<dyn ExternalToolSurfaceHandle> = registration.clone();
            move || adapter.bind_external_tool_surface_handle(registration)
        });
        // The bind is re-deriving the staged fact on the registration.
        tokio::task::spawn_blocking(move || entered.recv())
            .await
            .unwrap()
            .expect("the bind re-derives its pre-bind fact");
        adapter.shutdown().await;
        go.send(()).unwrap();
        binding.join().unwrap();
        assert_eq!(
            registration
                .surface_snapshot("raced")
                .map(|entry| entry.staged_op),
            Some(ExternalToolSurfaceStagedOp::None),
            "the ended router's re-derived staging is released"
        );
        assert_eq!(
            registration.diagnostic_snapshot().surface_phase,
            meerkat_core::ExternalToolSurfaceGlobalPhase::Operating
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_rejected_removal_boundary_keeps_the_governed_name() {
        let handle = Arc::new(StaleRemovalBoundary {
            inner: generated_surface_handle(),
            armed: AtomicBool::new(false),
        });
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            handle.clone(),
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        let server = test_server_config("governed", Path::new("/nonexistent/mcp-server"));
        let other = test_server_config("governed", Path::new("/nonexistent/other-server"));
        router
            .stage_add_governed(server.clone(), test_attachment(&server, Ok(())))
            .expect("stage under a prepared control");
        router.stage_remove("governed").expect("stage the removal");
        handle.armed.store(true, Ordering::SeqCst);
        let applied = router.apply_staged().await.expect("apply boundary");
        assert!(
            applied
                .delta
                .rejected_boundaries
                .iter()
                .any(|rejection| rejection.server == "governed"),
            "the stale removal boundary is rejected"
        );
        // The rejected removal did not dispose of the name.
        assert!(matches!(
            router.stage_add(other.clone()),
            Err(McpError::AttachmentRefused(McpAttachmentRefusal::Mismatch))
        ));
        // The newer removal's own accepted boundary disposes of it.
        router
            .apply_staged()
            .await
            .expect("apply the current removal");
        router
            .stage_add(other)
            .expect("an ungoverned add after the accepted removal");
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn an_active_governed_server_keeps_its_requirement_until_its_removal_finalizes() {
        let server_path = mcp_test_server::fixture_binary();
        let server = test_server_config("governed", &server_path);
        let mut replacement = server.clone();
        replacement.connect_timeout_secs = Some(30);
        let control = test_attachment(&server, Ok(()));
        let mut router = generated_surface_router();
        router
            .stage_add_governed(server, control.clone())
            .expect("stage under a prepared control");
        router.apply_staged().await.expect("apply the governed add");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("the governed server connects");
        assert_eq!(control.apply_checks.load(Ordering::SeqCst), 1);

        adapter
            .stage_remove("governed")
            .await
            .expect("stage the removal");
        let mismatch = McpAttachmentRefusal::Mismatch.to_string();
        let replaced = adapter
            .stage_add(replacement.clone())
            .await
            .expect_err("a plain add cannot overwrite the staged removal of a governed server");
        assert!(replaced.contains(&mismatch), "{replaced}");
        // Hold the removal in Removing with in-flight work: the name stays
        // governed after the applied boundary, until finalization.
        adapter
            .set_inflight_calls_for_testing("governed", 1)
            .await
            .expect("hold an in-flight call");
        adapter.apply_staged().await.expect("apply the removal");
        let held = adapter
            .stage_add(replacement.clone())
            .await
            .expect_err("still governed while the removal drains");
        assert!(held.contains(&mismatch), "{held}");
        adapter
            .set_inflight_calls_for_testing("governed", 0)
            .await
            .expect("release the in-flight call");
        adapter.apply_staged().await.expect("finalize the removal");
        assert!(
            adapter
                .wait_removals_finalized(async_connect_test_timeout())
                .await,
            "the removal finalizes"
        );
        adapter
            .stage_add(replacement)
            .await
            .expect("an ungoverned add after the removal finalized");
        adapter.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn the_immediate_add_path_refuses_a_governed_name() {
        let mut router = generated_surface_router();
        let server = test_server_config("governed", Path::new("/nonexistent/mcp-server"));
        let other = test_server_config("governed", Path::new("/nonexistent/other-server"));
        router
            .stage_add_governed(server.clone(), test_attachment(&server, Ok(())))
            .expect("stage under a prepared control");
        assert!(matches!(
            router.add_server(other).await,
            Err(McpError::AttachmentRefused(McpAttachmentRefusal::Mismatch))
        ));
        router.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_resolved_mcp_plan_carries_its_registration_and_raw_operation() {
        let server_path = mcp_test_server::fixture_binary();
        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("srv", &server_path))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("the server connects");
        let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
        let call = ToolCallView {
            id: "call-1",
            name: "echo",
            args: &args,
        };
        let plan = AgentToolDispatcher::resolve_execution_plan(
            &adapter,
            call,
            &meerkat_core::ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        Duration::from_secs(600),
                    ),
                ])
                .expect("deadline chain"),
            ),
        )
        .expect("a resolved plan");
        let witness = plan
            .owner_witness(&adapter.execution_authority_key())
            .expect("the adapter's witness");
        assert!(
            matches!(
                witness.operation_target(),
                Some(meerkat_core::ToolOperationTarget::McpServerOperation {
                    server,
                    raw_operation,
                    connector,
                    account,
                    ..
                }) if server == "srv"
                    && raw_operation == "echo"
                    && *connector == meerkat_core::McpConnectorApproval::NoConnector
                    && **account == meerkat_core::OperationAccount::Unavailable
            ),
            "the plan carries its exact registration, raw operation, connector approval \
             and account: a server that declares none is unavailable, never \"no account\""
        );
        {
            let guard = adapter.router.read().await;
            let router = guard.as_ref().expect("router");
            assert!(adapter.operation_target_matches(router, "echo", &plan));
            // A plan whose target is not the live router's is not dispatched.
            // (A synthetic stale-generation comparison, not a held
            // asynchronous replacement.)
            let stale = witness.clone().with_operation_target(
                meerkat_core::ToolOperationTarget::McpServerOperation {
                    server: "srv".to_string(),
                    connection_generation: u64::MAX,
                    raw_operation: "echo".to_string(),
                    connector: meerkat_core::McpConnectorApproval::NoConnector,
                    account: Arc::new(meerkat_core::OperationAccount::Unavailable),
                },
            );
            let stale_plan = meerkat_core::ToolExecutionContract::default()
                .resolve_default(
                    meerkat_core::ToolDeadlineChain::new(vec![
                        meerkat_core::ToolDeadlineContributor::finite(
                            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                            Duration::from_secs(600),
                        ),
                    ])
                    .expect("deadline chain"),
                )
                .expect("plan")
                .with_owner_witness(stale)
                .expect("stale witness");
            assert!(!adapter.operation_target_matches(router, "echo", &stale_plan));
        }
        adapter.shutdown().await;
    }

    /// The account rides on the exact live binding: a server whose host
    /// declares it acts with no account states `DeclaredNone` on its plans,
    /// and once the binding is replaced (here by the same server without the
    /// declaration) a plan resolved before the change is no longer the live
    /// router's and is not dispatched, while a fresh plan states the new
    /// binding's account. Nothing else records the account.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_changed_binding_changes_the_operations_account_and_retires_old_plans() {
        let server_path = mcp_test_server::fixture_binary();
        let mut declared = test_server_config("srv", &server_path);
        declared.account = Some(meerkat_core::McpAccountDeclaration::NoAccount);
        let mut router = generated_surface_router();
        router.stage_add(declared).expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("the server connects");
        let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
        let call = ToolCallView {
            id: "call-1",
            name: "echo",
            args: &args,
        };
        let resolve = || {
            AgentToolDispatcher::resolve_execution_plan(
                &adapter,
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &meerkat_core::ToolExecutionResolutionContext::new(
                    meerkat_core::ToolDeadlineChain::new(vec![
                        meerkat_core::ToolDeadlineContributor::finite(
                            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                            Duration::from_secs(600),
                        ),
                    ])
                    .expect("deadline chain"),
                ),
            )
            .expect("a resolved plan")
        };
        let account_of = |plan: &meerkat_core::ResolvedToolExecutionPlan| {
            plan.owner_witness(&adapter.execution_authority_key())
                .and_then(|witness| witness.operation_target())
                .and_then(|target| target.account().cloned())
        };
        let before = resolve();
        assert_eq!(
            account_of(&before),
            Some(meerkat_core::OperationAccount::DeclaredNone)
        );

        // The host replaces the binding: the same server, now undeclared.
        adapter.stage_remove("srv").await.expect("stage remove");
        adapter.apply_staged().await.expect("apply removal");
        adapter
            .stage_add(test_server_config("srv", &server_path))
            .await
            .expect("stage the replacement");
        adapter.apply_staged().await.expect("apply replacement");
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("the replacement connects");
        {
            let guard = adapter.router.read().await;
            let router = guard.as_ref().expect("router");
            assert!(
                !adapter.operation_target_matches(router, "echo", &before),
                "a plan resolved on the replaced binding is not the live router's"
            );
        }
        let after = resolve();
        assert_eq!(
            account_of(&after),
            Some(meerkat_core::OperationAccount::Unavailable),
            "the fresh plan states the live binding's account"
        );
        adapter.shutdown().await;
    }

    #[tokio::test]
    async fn empty_adapter_reports_exact_catalog_support() {
        let adapter = McpRouterAdapter::new(McpRouter::new_with_stdio_profile(
            crate::McpStdioLaunchProfile::trusted_host(),
        ));
        assert!(
            adapter.tool_catalog_capabilities().exact_catalog,
            "empty MCP adapters should preserve exact deferred catalogs on other tool planes"
        );
        assert!(adapter.tools().is_empty());
    }

    #[test]
    fn adapter_accepts_equivalent_projection_rebuilds_and_rejects_semantic_change() {
        let adapter = McpRouterAdapter::new(McpRouter::new_with_stdio_profile(
            crate::McpStdioLaunchProfile::trusted_host(),
        ));
        let entry = |description: &str| {
            ToolCatalogEntry::session_inline(
                Arc::new(ToolDef::new(
                    "echo",
                    description,
                    serde_json::json!({"type": "object"}),
                )),
                true,
            )
        };
        let original = entry("stable MCP binding");
        let rebuilt = entry("stable MCP binding");
        let changed = entry("replacement MCP binding");
        let plan = meerkat_core::ToolExecutionContract::default()
            .resolve_default(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        Duration::from_secs(600),
                    ),
                ])
                .expect("deadline chain"),
            )
            .expect("fast plan")
            .with_owner_witness(
                meerkat_core::ToolExecutionOwnerWitness::new(
                    adapter.execution_authority_key(),
                    "echo",
                    meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&original),
                )
                .expect("owner witness"),
            )
            .expect("attach witness");

        assert!(adapter.binding_witness_matches(
            "echo",
            Some(meerkat_core::ephemeral_tool_catalog_binding_fingerprint(
                &rebuilt,
            )),
            &plan,
        ));
        let refreshed_plan = meerkat_core::ToolExecutionContract::default()
            .resolve_default(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        Duration::from_secs(600),
                    ),
                ])
                .expect("deadline chain"),
            )
            .expect("fast plan")
            .with_owner_witness(
                meerkat_core::ToolExecutionOwnerWitness::new(
                    adapter.execution_authority_key(),
                    "echo",
                    meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&rebuilt),
                )
                .expect("fresh owner witness"),
            )
            .expect("attach fresh witness");
        assert!(adapter.binding_witness_matches(
            "echo",
            Some(meerkat_core::ephemeral_tool_catalog_binding_fingerprint(
                &rebuilt,
            )),
            &refreshed_plan,
        ));
        assert!(!adapter.binding_witness_matches(
            "echo",
            Some(meerkat_core::ephemeral_tool_catalog_binding_fingerprint(
                &changed,
            )),
            &plan,
        ));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn default_router_fails_closed_until_external_surface_handle_is_bound() {
        let mut router =
            McpRouter::new_with_stdio_profile(crate::McpStdioLaunchProfile::trusted_host());
        let error = router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "planner",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .expect_err("default router must reject lifecycle mutation before generated bind");
        assert!(
            error.to_string().contains("generated machine authority"),
            "expected generated authority rejection, got {error}"
        );

        let adapter = McpRouterAdapter::new(router);
        let handle = generated_surface_handle();

        adapter.bind_external_tool_surface_handle(Arc::clone(&handle));
        adapter
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "planner",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .await
            .expect("stage planner after generated bind");
        adapter.apply_staged().await.expect("apply staged add");

        let snapshot = handle
            .surface_snapshot("planner")
            .expect("planner surface snapshot");
        assert_eq!(snapshot.pending_op, ExternalToolSurfacePendingOp::Add);
        assert_eq!(snapshot.pending_task_sequence, Some(1));
        assert_eq!(snapshot.pending_lineage_sequence, Some(1));
    }

    /// 0.7.19 regression (meerkat-studio): the composite dispatcher now
    /// forwards the session-authority bind to nested MCP adapters, so the
    /// standard embedder flow — construct the router on an ephemeral
    /// authority, stage + connect tool servers, THEN build the agent (bind
    /// arrives) — hit the pre-bind poison and every subsequent tool call
    /// failed with "surface is poisoned … refusing to replay handwritten
    /// snapshot". Pre-bind facts must instead re-derive onto the session
    /// authority and calls must keep working after the bind.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn adapter_late_bind_rederives_pre_bind_facts_instead_of_poisoning() {
        let server_path = mcp_test_server::fixture_binary();
        // Embedder flow: ephemeral construction-time authority, servers
        // staged + applied + connected BEFORE the session bind exists.
        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("desktop_ui", &server_path))
            .expect("stage desktop_ui pre-bind");
        router.apply_staged().await.expect("apply staged add");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .wait_until_ready(std::time::Duration::from_secs(10))
            .await
            .expect("pre-bind server should connect");
        adapter.refresh_tools().await.expect("refresh tools");

        // The agent build delivers the session authority late.
        let session_handle = generated_surface_handle();
        adapter.bind_external_tool_surface_handle(Arc::clone(&session_handle));

        // The session authority now owns the re-derived facts (the surface
        // completion that connected pre-bind drains post-bind and must land
        // on the session authority with matching machine sequences).
        let _ = AgentToolDispatcher::poll_external_updates(&adapter).await;
        let seeded = session_handle
            .surface_snapshot("desktop_ui")
            .expect("session authority must own the re-derived surface");
        assert_eq!(
            seeded.base_state,
            Some(meerkat_core::ExternalToolSurfaceBaseState::Active),
            "re-derived surface must be Active on the session authority: {seeded:?}"
        );

        // And calls keep working — the exact operation that failed in the
        // field with the poisoned handle.
        let tools = AgentToolDispatcher::tools(&adapter);
        let tool = tools
            .iter()
            .find(|tool| tool.name.as_str() == "echo")
            .expect("connected server must expose tools after the late bind");
        let args =
            serde_json::value::RawValue::from_string(r#"{"message":"after-bind"}"#.to_string())
                .expect("raw args");
        let tool_name = tool.name.to_string();
        let view = ToolCallView {
            id: "call-after-bind",
            name: &tool_name,
            args: &args,
        };
        let outcome = AgentToolDispatcher::dispatch(&adapter, view).await;
        assert!(
            outcome.is_ok(),
            "tool calls must keep working after the late session bind: {outcome:?}"
        );

        // Post-bind surface mutations flow through the session authority.
        adapter
            .stage_add(test_server_config("post-bind", &server_path))
            .await
            .expect("post-bind stage_add must not be poisoned");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn adapter_bind_external_surface_handle_failure_poisons_existing_owner() {
        let mut router = generated_surface_router();
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "planner",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .expect("stage planner");
        let adapter = McpRouterAdapter::new(router);
        let rejecting_handle: Arc<dyn ExternalToolSurfaceHandle> =
            Arc::new(RejectingExternalToolSurfaceHandle);

        adapter.bind_external_tool_surface_handle(rejecting_handle);

        let error = adapter
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "writer",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .await
            .expect_err("poisoned bind should fail future surface mutation");
        assert!(
            error.contains("surface is poisoned"),
            "expected poisoned surface error, got {error}"
        );

        let snapshot = adapter
            .external_tool_surface_snapshot()
            .expect("poisoned adapter preserves diagnostic snapshot");
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].surface_id, "planner");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn adapter_bind_external_surface_handle_allows_post_bind_pending_state() {
        let server_path = mcp_test_server::fixture_binary();
        let router =
            McpRouter::new_with_stdio_profile(crate::McpStdioLaunchProfile::trusted_host());
        let adapter = McpRouterAdapter::new(router);
        let handle = generated_surface_handle();

        adapter.bind_external_tool_surface_handle(Arc::clone(&handle));
        adapter
            .stage_add(test_server_config("late-bind-pending", &server_path))
            .await
            .expect("stage server after generated bind");
        adapter
            .apply_staged()
            .await
            .expect("apply staged after bind");
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("server should connect before the wait deadline");

        let snapshot = handle
            .surface_snapshot("late-bind-pending")
            .expect("late-bound surface snapshot");
        assert_eq!(
            snapshot.base_state,
            Some(ExternalToolSurfaceBaseState::Active)
        );
        assert_eq!(snapshot.pending_op, ExternalToolSurfacePendingOp::None);
        assert!(
            handle.visible_surfaces().contains("late-bind-pending"),
            "pending completion should land on the late-bound surface owner"
        );
    }

    // Mock handle used to observe Phase 5G / T5g MCP server lifecycle events
    // routed through `McpServerLifecycleHandle`.
    struct MockMcpServerLifecycleHandle {
        events: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl MockMcpServerLifecycleHandle {
        fn new() -> Self {
            Self {
                events: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn push(&self, kind: &str, server_id: &str) {
            self.events
                .lock()
                .unwrap()
                .push((kind.to_string(), server_id.to_string()));
        }

        fn events(&self) -> Vec<(String, String)> {
            self.events.lock().unwrap().clone()
        }
    }

    impl meerkat_core::handles::McpServerLifecycleHandle for MockMcpServerLifecycleHandle {
        fn apply_connect_pending(
            &self,
            server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.push("connect_pending", server_id);
            Ok(())
        }
        fn apply_connected(
            &self,
            server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.push("connected", server_id);
            Ok(())
        }
        fn apply_failed(
            &self,
            server_id: &str,
            _error: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.push("failed", server_id);
            Ok(())
        }
        fn apply_disconnected(
            &self,
            server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.push("disconnected", server_id);
            Ok(())
        }
        fn apply_reload(
            &self,
            server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            self.push("reload", server_id);
            Ok(())
        }
        fn pending_server_ids(&self) -> BTreeSet<String> {
            std::collections::BTreeSet::new()
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn router_stage_add_fires_connect_pending_on_bound_lifecycle_handle() {
        let mut router = generated_surface_router();
        let handle = Arc::new(MockMcpServerLifecycleHandle::new());
        // Bind via the shared slot (the path the adapter uses internally).
        let handle_slot = router.mcp_lifecycle_handle_slot();
        {
            let mut slot = handle_slot.write().unwrap();
            *slot = Some(
                Arc::clone(&handle) as Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>
            );
        }

        let cfg = meerkat_core::McpServerConfig::stdio(
            "srv-alpha",
            "/bin/echo",
            Vec::<String>::new(),
            HashMap::new(),
        );
        router.stage_add(cfg).expect("stage add");

        let events = handle.events();
        assert_eq!(
            events,
            vec![("connect_pending".to_string(), "srv-alpha".to_string())],
            "stage_add on a fresh router should fire exactly apply_connect_pending"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn adapter_bind_seeds_pre_bind_pending_servers_into_dsl() {
        let server_path = mcp_test_server::fixture_binary();
        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("srv-beta", &server_path))
            .expect("stage add");
        // Simulate the CLI flow: stage_add + apply_staged run before the
        // agent-factory bind happens. After apply_staged, pending_sources
        // reflects the in-flight background connect tasks.
        router.apply_staged().await.expect("apply staged");

        let adapter = McpRouterAdapter::new(router);
        // Adapter construction captured pending_sources_snapshot via
        // the cached_pending_sources buffer; that's what bind seeds from.
        let handle = Arc::new(MockMcpServerLifecycleHandle::new());
        adapter.bind_mcp_server_lifecycle_handle(
            Arc::clone(&handle) as Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>
        );

        let events = handle.events();
        assert!(
            events
                .iter()
                .any(|(kind, name)| kind == "connect_pending" && name == "srv-beta"),
            "bind should seed apply_connect_pending for pre-bind pending servers, got {events:?}"
        );

        adapter.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn filtered_adapter_forwards_mcp_lifecycle_handle_to_retained_router() {
        let adapter = Arc::new(McpRouterAdapter::new(generated_surface_router()));
        let filtered =
            meerkat_core::FilteredToolDispatcher::new(Arc::clone(&adapter), Vec::<String>::new());
        let handle = Arc::new(MockMcpServerLifecycleHandle::new());

        filtered.bind_mcp_server_lifecycle_handle(
            Arc::clone(&handle) as Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>
        );
        adapter
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "filtered-retained",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .await
            .expect("filtered wrapper must retain lifecycle authority on the adapter");

        assert_eq!(
            handle.events(),
            vec![(
                "connect_pending".to_string(),
                "filtered-retained".to_string(),
            )],
            "a filtered MCP adapter must forward handshake lifecycle events to the session handle"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn router_lifecycle_handle_none_is_noop() {
        // Router without a bound MCP lifecycle mirror: external surface
        // lifecycle still flows through its owner, while mirror notifications
        // remain no-ops.
        let handle = generated_surface_handle();
        handle
            .stage_add("srv-standalone".to_string(), 0)
            .expect("seed stage add");
        let staged_sequence = handle
            .surface_snapshot("srv-standalone")
            .and_then(|snapshot| snapshot.staged_intent_sequence)
            .expect("seed staged sequence");
        handle
            .apply_boundary(
                "srv-standalone".to_string(),
                0,
                staged_sequence,
                staged_sequence,
            )
            .expect("seed apply boundary");
        let pending_sequence = handle
            .surface_snapshot("srv-standalone")
            .and_then(|snapshot| snapshot.pending_task_sequence)
            .expect("seed pending sequence");
        handle
            .mark_pending_succeeded(
                "srv-standalone".to_string(),
                pending_sequence,
                staged_sequence,
            )
            .expect("seed active surface");
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            handle,
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        let cfg = meerkat_core::McpServerConfig::stdio(
            "srv-standalone",
            "/bin/echo",
            Vec::<String>::new(),
            HashMap::new(),
        );
        router.stage_add(cfg).expect("stage add");
        router.stage_reload("srv-standalone").expect("stage reload");
        // Nothing panicked; standalone path remains intact.
    }

    #[tokio::test]
    async fn pending_empty_adapter_keeps_exact_catalog_support_and_reports_pending_sources() {
        let adapter = McpRouterAdapter::new(McpRouter::new_with_stdio_profile(
            crate::McpStdioLaunchProfile::trusted_host(),
        ));
        adapter.has_pending.store(true, Ordering::Release);
        adapter.set_pending_sources_cache(Arc::from(["pending-mcp".to_string()]));

        assert!(
            adapter.tool_catalog_capabilities().exact_catalog,
            "empty pending MCP adapters should preserve exact catalog support on sibling planes"
        );
        assert_eq!(
            adapter.pending_catalog_sources().as_ref(),
            ["pending-mcp".to_string()].as_slice()
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn wait_until_ready_returns_notices_when_server_connects() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("test-srv", &server_path))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");

        let adapter = McpRouterAdapter::new(router);

        let notices = adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("server should connect before the wait deadline");

        // Server should have connected successfully.
        assert!(
            !notices.is_empty(),
            "expected at least one notice from the connecting server"
        );
        assert!(
            notices.iter().any(|n| n.target == "test-srv"),
            "notice should reference the staged server"
        );

        // After waiting, pending should be empty.
        let update = adapter.poll_external_updates().await;
        assert!(
            update.pending.is_empty(),
            "no servers should be pending after wait_until_ready"
        );

        adapter.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn wait_until_ready_pending_at_deadline_is_typed_mcp_not_ready() {
        // K14: a server still connecting at the deadline must surface as the
        // typed `McpNotReady` fault (carrying the pending set), never as a
        // warn-and-proceed empty success. (Fails-old: the deadline arm broke
        // out of the loop with a `tracing::warn!` and returned notices as if
        // the wait had succeeded.)
        let mut router = generated_surface_router();
        // `/bin/sleep` accepts the stdio handshake write but never answers,
        // so the connect task stays pending past the wait deadline.
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "hang-srv",
                "/bin/sleep",
                vec!["60".to_string()],
                HashMap::new(),
            ))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);

        let not_ready = adapter
            .wait_until_ready(Duration::from_millis(200))
            .await
            .expect_err("hanging server must fail the wait typed");
        assert_eq!(
            not_ready.pending,
            vec!["hang-srv".to_string()],
            "the typed fault must carry the pending server set"
        );

        adapter.shutdown().await;
    }

    /// Shutdown owns in-flight connects: shutting down the adapter while a
    /// stdio server is still connecting kills that server, which has exited
    /// when shutdown returns. (Fails-old: shutdown never joined the spawned
    /// connect task, which kept the child alive until the connect timeout; a
    /// stopping test runtime then skipped the deferred kill entirely and left
    /// the child holding the test's stderr. After the join, the kill was still
    /// fire-and-forget, so the child outlived shutdown.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_kills_a_stdio_server_still_connecting() {
        use crate::stdio_test_fixture::PidReport;
        let mut report = PidReport::new("connect-join");
        let mut router = generated_surface_router();
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "hang-srv",
                "/bin/sh",
                vec![
                    "-c".to_string(),
                    format!("echo $$ > '{}'; exec sleep 60", report.path().display()),
                ],
                HashMap::new(),
            ))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        let pid = report.pids().await[0];

        adapter.shutdown().await;

        crate::connection::tests::assert_child_reaped(pid);
    }

    /// Stdio servers often launch through a wrapper (`sh -c`, `npx`, `uvx`),
    /// so the real server is a grandchild. Shutdown terminates the server's
    /// whole process group, so the grandchild cannot be orphaned.
    /// (Fails-old: only the direct child was killed.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_kills_a_wrapped_stdio_servers_whole_process_group() {
        use crate::stdio_test_fixture::{PidReport, process_exited};
        let mut report = PidReport::new("wrapped-group");
        let mut router = generated_surface_router();
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "wrapped-srv",
                "/bin/sh",
                vec![
                    "-c".to_string(),
                    format!(
                        "sleep 60 & echo $$ $! > '{}'; wait",
                        report.path().display()
                    ),
                ],
                HashMap::new(),
            ))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        let pids = report.pids().await;
        let (wrapper, server) = (pids[0], pids[1]);

        adapter.shutdown().await;

        crate::connection::tests::assert_child_reaped(wrapper);
        assert!(
            process_exited(server),
            "wrapped server {server} (a grandchild) outlived adapter shutdown"
        );
    }

    /// An established wrapped server that keeps running past stdin EOF is
    /// terminated with its whole process group once its close grace ends, and
    /// has exited when shutdown returns. (Fails-old: rmcp gave the wrapper up
    /// to 3 s after EOF from a detached task, then killed only the wrapper.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_kills_an_established_wrapped_stdio_servers_process_group() {
        use crate::stdio_test_fixture::{PidReport, process_exited, sh_mcp_server_args};
        let mut report = PidReport::new("established-group");
        let mut router = generated_surface_router();
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "established-srv",
                "/bin/sh",
                sh_mcp_server_args(Some(report.path())),
                HashMap::new(),
            ))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        let pids = report.pids().await;
        let (wrapper, server) = (pids[0], pids[1]);
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("fixture server completes the handshake");

        adapter.shutdown().await;

        crate::connection::tests::assert_child_reaped(wrapper);
        assert!(
            process_exited(server),
            "established server's grandchild {server} outlived adapter shutdown"
        );
    }

    /// A shutdown dropped while its router is still closing servers does not
    /// cut that work short, and a later shutdown waits for the same work
    /// before returning. (Fails-old: the later call found no router and
    /// returned while the servers were still in their close grace.)
    #[cfg(feature = "workspace-tests")]
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_repeated_shutdown_joins_the_dropped_one() {
        use crate::stdio_test_fixture::{PidReport, process_exited, sh_mcp_server_args};
        let mut report = PidReport::new("repeat-shutdown");
        let bounds = crate::McpStdioCloseBounds::new(
            std::time::Duration::from_millis(500),
            std::time::Duration::from_secs(10),
        )
        .expect("bounds within the maximum");
        let mut router = generated_surface_router().with_stdio_close_bounds(bounds);
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "eof-ignoring",
                "/bin/sh",
                sh_mcp_server_args(Some(report.path())),
                HashMap::new(),
            ))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        let pids = report.pids().await;
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("fixture server completes the handshake");

        let dropped =
            tokio::time::timeout(std::time::Duration::from_millis(100), adapter.shutdown()).await;
        assert!(dropped.is_err(), "the server was still in its EOF grace");
        adapter.shutdown().await;

        for pid in pids {
            assert!(
                process_exited(pid),
                "stdio process {pid} outlived the repeated shutdown"
            );
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn stage_reload_all_returns_typed_report_and_fails_closed_on_shutdown() {
        let server_path = mcp_test_server::fixture_binary();
        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("reload-srv", &server_path))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("server should connect before the wait deadline");

        // K14: one reload primitive stages the whole active roster under a
        // single router lock and returns the typed per-server report.
        let report = adapter
            .stage_reload_all()
            .await
            .expect("reload-all on a live adapter");
        assert!(
            report.is_clean(),
            "unexpected failures: {:?}",
            report.failed
        );
        assert_eq!(report.staged, vec!["reload-srv".to_string()]);

        adapter.shutdown().await;
        let error = adapter
            .stage_reload_all()
            .await
            .expect_err("reload-all after shutdown must fail typed");
        assert!(matches!(error, crate::McpError::RouterShutDown));
    }

    // Rejecting MCP server-lifecycle mirror used to pin K14 fail-closed
    // behavior.
    struct RejectingMcpServerLifecycleHandle;

    impl meerkat_core::handles::McpServerLifecycleHandle for RejectingMcpServerLifecycleHandle {
        fn apply_connect_pending(
            &self,
            _server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                "RejectingMcpServerLifecycleHandle::apply_connect_pending",
                "injected lifecycle mirror rejection",
            ))
        }
        fn apply_connected(
            &self,
            _server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                "RejectingMcpServerLifecycleHandle::apply_connected",
                "injected lifecycle mirror rejection",
            ))
        }
        fn apply_failed(
            &self,
            _server_id: &str,
            _error: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                "RejectingMcpServerLifecycleHandle::apply_failed",
                "injected lifecycle mirror rejection",
            ))
        }
        fn apply_disconnected(
            &self,
            _server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                "RejectingMcpServerLifecycleHandle::apply_disconnected",
                "injected lifecycle mirror rejection",
            ))
        }
        fn apply_reload(
            &self,
            _server_id: &str,
        ) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Err(meerkat_core::handles::DslTransitionError::guard_rejected(
                "RejectingMcpServerLifecycleHandle::apply_reload",
                "injected lifecycle mirror rejection",
            ))
        }
        fn pending_server_ids(&self) -> BTreeSet<String> {
            std::collections::BTreeSet::new()
        }
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn retry_after_shutdown_is_router_closed_and_reinstalls_nothing() {
        let (router, target, resolver, _server) =
            crate::router::tests::router_awaiting_authorization().await;
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        let adapter = Arc::new(McpRouterAdapter::new(router));
        // A detached snapshot of the adapter, as a session-map read takes.
        let snapshot = Arc::clone(&adapter);
        adapter.shutdown().await;
        assert_eq!(
            snapshot.retry_authorized(&target).await.unwrap(),
            crate::AuthorizedRetry::RouterClosed
        );
        assert!(snapshot.active_server_names().await.is_empty());
        assert!(snapshot.servers_awaiting_authorization().await.is_empty());
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn retry_staged_before_shutdown_is_discarded_by_shutdown() {
        let (router, target, resolver, _server) =
            crate::router::tests::router_awaiting_authorization().await;
        *resolver.stored_token_for_testing() = Some("interactive-token".into());
        let adapter = Arc::new(McpRouterAdapter::new(router));
        let snapshot = Arc::clone(&adapter);
        assert_eq!(
            snapshot.retry_authorized(&target).await.unwrap(),
            crate::AuthorizedRetry::Staged
        );
        adapter.shutdown().await;
        // The staged retry died with the router; nothing applies it later.
        assert!(snapshot.apply_staged().await.is_err());
        assert!(snapshot.active_server_names().await.is_empty());
        assert_eq!(
            snapshot.retry_authorized(&target).await.unwrap(),
            crate::AuthorizedRetry::RouterClosed
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn stage_add_rejected_lifecycle_mirror_propagates_and_leaves_shell_unchanged() {
        // K14: a rejected lifecycle mirror apply is a typed `McpError::
        // LifecycleMirrorRejected` and the staged payload is NOT installed —
        // the staged intent then fails explicitly at apply_staged instead of
        // silently proceeding with a diverged mirror. (Fails-old: the
        // rejection was `tracing::debug!`-swallowed and staging proceeded.)
        let mut router = generated_surface_router();
        let handle_slot = router.mcp_lifecycle_handle_slot();
        {
            let mut slot = handle_slot.write().unwrap();
            *slot = Some(Arc::new(RejectingMcpServerLifecycleHandle)
                as Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>);
        }

        let error = router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "srv-rejected",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .expect_err("rejected lifecycle mirror apply must propagate typed");
        assert!(
            matches!(
                &error,
                crate::McpError::LifecycleMirrorRejected { server, .. } if server == "srv-rejected"
            ),
            "expected LifecycleMirrorRejected, got {error}"
        );

        // The shell mirror was not mutated: the staged intent exists on the
        // owner but carries no payload, so the boundary apply surfaces the
        // explicit missing-payload protocol fault instead of connecting.
        let apply_error = router
            .apply_staged()
            .await
            .expect_err("staged intent without payload must fail explicitly");
        assert!(
            apply_error
                .to_string()
                .contains("missing its staged payload"),
            "expected missing-payload protocol error, got {apply_error}"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn bind_seed_rejection_poisons_lifecycle_mirror_fail_closed() {
        // K14: a rejected pre-bind seed apply poisons the lifecycle mirror —
        // subsequent lifecycle applies fail typed and the seeded servers stay
        // visibly pending — instead of being `tracing::debug!`-swallowed and
        // binding a silently diverged handle. (Fails-old: the seed rejection
        // was logged at debug level and the handle was bound anyway.)
        let adapter = McpRouterAdapter::new(generated_surface_router());
        adapter.set_pending_sources_cache(Arc::from(["pre-bind-srv".to_string()]));

        adapter.bind_mcp_server_lifecycle_handle(Arc::new(RejectingMcpServerLifecycleHandle)
            as Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>);

        let bound = adapter
            .mcp_lifecycle_handle
            .read()
            .unwrap()
            .clone()
            .expect("bind installs a handle");
        assert_eq!(
            bound.pending_server_ids(),
            std::iter::once("pre-bind-srv".to_string()).collect(),
            "poisoned mirror must keep the seeded servers visibly pending"
        );
        let apply_error = bound
            .apply_connected("pre-bind-srv")
            .expect_err("poisoned mirror must reject subsequent applies typed");
        assert!(
            apply_error.to_string().contains("poisoned"),
            "expected poisoned mirror rejection, got {apply_error}"
        );

        let stage_error = adapter
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "post-poison-srv",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .await
            .expect_err("staging through a poisoned mirror must fail typed");
        assert!(
            stage_error.contains("lifecycle mirror"),
            "expected lifecycle-mirror rejection through the adapter, got {stage_error}"
        );
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn connected_adapter_reports_exact_catalog_support_with_deferred_entries() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_surface_router();
        router
            .add_server(test_server_config("exact-srv", &server_path))
            .await
            .expect("add server");

        let adapter = McpRouterAdapter::new(router);
        let catalog = adapter.tool_catalog();

        assert!(
            adapter.tool_catalog_capabilities().exact_catalog,
            "connected MCP adapters should keep exact catalog support"
        );
        assert!(
            !catalog.is_empty(),
            "connected MCP adapters should publish a canonical catalog"
        );
        assert!(
            catalog.iter().all(|entry| matches!(
                entry.deferred_eligibility,
                meerkat_core::ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )),
            "connected MCP catalog entries should stay deferred-eligible when provenance proves stable ownership"
        );
        assert_eq!(
            adapter
                .tools()
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
            catalog
                .iter()
                .map(|entry| entry.tool.name.clone())
                .collect::<Vec<_>>(),
            "adapter tools() should expose the same canonical winner set as its exact catalog"
        );

        adapter.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn mcp_reload_rebuild_requires_fresh_live_resolution() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("stable-reload", &server_path))
            .expect("stage server");
        router.apply_staged().await.expect("apply server");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("server connects");
        adapter.refresh_tools().await.expect("refresh tools");

        let tool_name = adapter
            .tools()
            .iter()
            .find(|tool| tool.name.as_str() == "echo")
            .expect("test server exposes echo")
            .name
            .to_string();
        let args =
            serde_json::value::RawValue::from_string(r#"{"message":"same-binding"}"#.to_string())
                .expect("raw arguments");
        let call = ToolCallView {
            id: "stable-mcp-binding",
            name: &tool_name,
            args: &args,
        };
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    Duration::from_secs(600),
                ),
            ])
            .expect("deadline chain"),
        );
        let plan = adapter
            .resolve_execution_plan(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("initial projection resolves");

        adapter
            .stage_reload("stable-reload")
            .await
            .expect("stage reload");
        adapter.apply_staged().await.expect("apply reload");
        adapter
            .wait_until_ready(async_connect_test_timeout())
            .await
            .expect("reloaded server connects");
        adapter
            .refresh_tools()
            .await
            .expect("refresh reloaded tools");

        let error = adapter
            .dispatch_resolved_with_context(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &plan,
            )
            .await
            .expect_err("turn-local MCP witnesses must not survive a router reload");
        assert!(matches!(
            error,
            ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            }
        ));
        let refreshed = adapter
            .resolve_execution_plan(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("reload path re-resolves against the live router");
        adapter
            .dispatch_resolved_with_context(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &refreshed,
            )
            .await
            .expect("fresh resolution must allow the reconstructed MCP binding");

        adapter.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn lifecycle_action_reload_invalidates_plan_resolved_while_connection_was_pending() {
        let server_path = mcp_test_server::fixture_binary();

        let mut router = generated_surface_router();
        router
            .add_server(test_server_config("lifecycle-reload", &server_path))
            .await
            .expect("initial server connects");
        let adapter = McpRouterAdapter::new(router);
        adapter
            .poll_lifecycle_actions()
            .await
            .expect("drain initial add lifecycle action");
        let args =
            serde_json::value::RawValue::from_string(r#"{"message":"lifecycle"}"#.to_string())
                .expect("raw arguments");
        let call = ToolCallView {
            id: "lifecycle-reload-binding",
            name: "echo",
            args: &args,
        };
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    Duration::from_secs(600),
                ),
            ])
            .expect("deadline chain"),
        );

        adapter
            .stage_reload("lifecycle-reload")
            .await
            .expect("stage reload");
        adapter.apply_staged().await.expect("start pending reload");
        let pending_plan = adapter
            .resolve_execution_plan(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("old live connection remains resolvable while reload is pending");

        let actions = tokio::time::timeout(async_connect_test_timeout(), async {
            loop {
                let actions = adapter
                    .poll_lifecycle_actions()
                    .await
                    .expect("poll lifecycle actions");
                if !actions.is_empty() {
                    break actions;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("reload lifecycle action arrives");
        assert!(actions.iter().any(|action| {
            action.target == "lifecycle-reload"
                && action.operation == meerkat_core::ToolConfigChangeOperation::Reload
                && action.phase == meerkat_core::ExternalToolDeltaPhase::Applied
        }));

        let error = adapter
            .dispatch_resolved_with_context(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &pending_plan,
            )
            .await
            .expect_err("lifecycle installation must invalidate the pending-era plan");
        assert!(matches!(
            error,
            ToolError::Unavailable {
                reason: meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            }
        ));

        let fresh = adapter
            .resolve_execution_plan(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("fresh connection resolves after lifecycle installation");
        adapter
            .dispatch_resolved_with_context(
                call,
                &meerkat_core::ToolDispatchContext::default(),
                &fresh,
            )
            .await
            .expect("fresh connection dispatches");
        adapter.shutdown().await;
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn adapter_fails_closed_instead_of_authorizing_cached_contract_under_router_contention() {
        let adapter = McpRouterAdapter::new(generated_surface_router());
        let detached = meerkat_core::DetachedToolExecutionPolicy::new(
            meerkat_core::RunnerIdentity::new("test.mcp", "v1").expect("runner declaration"),
            meerkat_core::RestartClass::NonResumable,
            meerkat_core::IdempotencyScope::ToolCall,
            Duration::from_secs(10),
        )
        .expect("detached policy");
        let contract = meerkat_core::ToolExecutionContract::new(
            BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
            meerkat_core::ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("detached execution contract");
        adapter.set_catalog_cache(
            vec![
                ToolCatalogEntry::session_inline(
                    Arc::new(ToolDef::new(
                        "cached_detached_mcp",
                        "cached detached MCP tool",
                        serde_json::json!({"type": "object"}),
                    )),
                    true,
                )
                .with_execution_contract(contract),
            ]
            .into(),
        );
        let raw =
            serde_json::value::RawValue::from_string("{}".to_string()).expect("raw arguments");
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    Duration::from_secs(600),
                ),
            ])
            .expect("deadline chain"),
        );
        let _router_write_guard = adapter.router.write().await;

        let error = adapter
            .resolve_execution_plan(
                ToolCallView {
                    id: "mcp-cached-plan",
                    name: "cached_detached_mcp",
                    args: raw.as_ref(),
                },
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect_err("a stale cache must never authorize while live routing is contended");

        assert!(matches!(
            error,
            meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name,
                reason: meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
            } if tool_name == "cached_detached_mcp"
        ));
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn external_tool_surface_snapshot_reflects_staged_surface_state() {
        let mut router = generated_surface_router();
        router
            .stage_add(test_server_config("planner", Path::new("/bin/echo")))
            .expect("stage add");

        let adapter = McpRouterAdapter::new(router);
        let snapshot = adapter
            .external_tool_surface_snapshot()
            .expect("router-backed adapter should expose surface snapshot");

        assert_eq!(
            snapshot.phase,
            meerkat_core::ExternalToolSurfaceGlobalPhase::Operating
        );
        assert_eq!(snapshot.snapshot_epoch, 0);
        assert_eq!(snapshot.snapshot_aligned_epoch, 0);
        assert_eq!(snapshot.entries.len(), 1);

        let entry = &snapshot.entries[0];
        assert_eq!(entry.surface_id, "planner");
        assert!(!entry.visible);
        assert_eq!(
            entry.base_state,
            meerkat_core::ExternalToolSurfaceBaseState::Absent
        );
        assert_eq!(
            entry.pending_op,
            meerkat_core::ExternalToolSurfacePendingOp::None
        );
        assert_eq!(
            entry.staged_op,
            meerkat_core::ExternalToolSurfaceStagedOp::Add
        );
        assert_eq!(entry.staged_intent_sequence, 1);
        assert_eq!(entry.pending_task_sequence, 0);
        assert_eq!(entry.pending_lineage_sequence, 0);
        assert_eq!(entry.inflight_call_count, 0);
        assert_eq!(
            entry.last_delta_operation,
            meerkat_core::ExternalToolSurfaceDeltaOperation::None
        );
        assert_eq!(
            entry.last_delta_phase,
            meerkat_core::ExternalToolSurfaceDeltaPhase::None
        );
    }

    /// The exact attempt of one governed live add (ADR membermcp observe
    /// review r1): only that attempt's own accepted outcome answers it.
    #[cfg(target_os = "linux")]
    #[allow(clippy::panic)]
    mod attempt_observation {
        use super::*;
        use meerkat_core::mcp_attachment::McpAttachmentReadiness;

        fn listing_server(name: &str, tools: &[&str]) -> meerkat_core::McpServerConfig {
            meerkat_core::McpServerConfig::stdio(
                name,
                "/bin/sh".to_string(),
                crate::stdio_test_fixture::sh_mcp_server_args_listing("", tools),
                HashMap::new(),
            )
        }

        fn shell_server(name: &str, script: &str) -> meerkat_core::McpServerConfig {
            meerkat_core::McpServerConfig::stdio(
                name,
                "/bin/sh".to_string(),
                vec!["-c".to_string(), script.to_string()],
                HashMap::new(),
            )
        }

        async fn stage(
            adapter: &Arc<McpRouterAdapter>,
            server: &meerkat_core::McpServerConfig,
        ) -> meerkat_core::mcp_attachment::McpAttachmentAttempt {
            adapter
                .stage_add_governed_observed(server.clone(), test_attachment(server, Ok(())))
                .await
                .expect("stage under a prepared control")
        }

        /// Usable only once the attempt's own connection is installed, and
        /// only for an operation its published routes serve; the lifecycle
        /// notice of that install is still delivered to the normal poll.
        #[cfg(feature = "workspace-tests")]
        #[tokio::test]
        async fn an_installed_attempt_is_usable_only_for_its_routed_operation() {
            let adapter = Arc::new(McpRouterAdapter::new(generated_surface_router()));
            let server = listing_server("tools", &["echo"]);
            let attempt = stage(&adapter, &server).await;
            assert_eq!(
                attempt.observe("echo").await,
                McpAttachmentReadiness::Pending
            );
            adapter.apply_staged().await.expect("apply boundary");
            let ready = attempt.wait("echo", async_connect_test_timeout()).await;
            let McpAttachmentReadiness::Usable {
                connection_generation,
            } = ready
            else {
                panic!("the installed attempt serves its operation: {ready:?}");
            };
            assert_eq!(
                attempt.observe("missing").await,
                McpAttachmentReadiness::ToolNotRouted {
                    connection_generation
                }
            );
            let update = adapter.poll_external_updates().await;
            assert!(
                update.notices.iter().any(|notice| notice.target == "tools"),
                "the wait left the install notice for the normal delivery: {:?}",
                update.notices
            );
            adapter.shutdown().await;
            assert_eq!(
                attempt.observe("echo").await,
                McpAttachmentReadiness::Retired
            );
        }

        /// A connection that never completes the handshake is the attempt's
        /// own accepted failure, never usable.
        #[cfg(feature = "workspace-tests")]
        #[tokio::test]
        async fn a_failed_attempt_is_failed_and_never_usable() {
            let adapter = Arc::new(McpRouterAdapter::new(generated_surface_router()));
            let attempt = stage(&adapter, &shell_server("broken", "exit 3")).await;
            adapter.apply_staged().await.expect("apply boundary");
            let outcome = attempt.wait("echo", async_connect_test_timeout()).await;
            assert!(
                matches!(outcome, McpAttachmentReadiness::Failed { .. }),
                "{outcome:?}"
            );
            assert_eq!(attempt.observe("echo").await, outcome);
        }

        /// An attempt replaced before its boundary is superseded, and stays
        /// so when the replacement's same-name connection becomes usable.
        #[cfg(feature = "workspace-tests")]
        #[tokio::test]
        async fn an_attempt_replaced_before_its_boundary_is_superseded() {
            let adapter = Arc::new(McpRouterAdapter::new(generated_surface_router()));
            let first = stage(&adapter, &listing_server("same", &["echo"])).await;
            let second = stage(&adapter, &listing_server("same", &["echo"])).await;
            assert_eq!(
                first.observe("echo").await,
                McpAttachmentReadiness::Superseded
            );
            adapter.apply_staged().await.expect("apply boundary");
            assert!(matches!(
                second.wait("echo", async_connect_test_timeout()).await,
                McpAttachmentReadiness::Usable { .. }
            ));
            assert_eq!(
                first.observe("echo").await,
                McpAttachmentReadiness::Superseded
            );
        }

        /// The wait deadline bounds router lock acquisition too: a router
        /// held elsewhere answers Pending at the deadline instead of waiting
        /// on the lock.
        #[cfg(feature = "workspace-tests")]
        #[tokio::test]
        async fn a_wait_deadline_bounds_router_lock_acquisition() {
            let adapter = Arc::new(McpRouterAdapter::new(generated_surface_router()));
            let attempt = stage(&adapter, &listing_server("held", &["echo"])).await;
            let held = adapter.router.write().await;
            let answered = tokio::time::timeout(
                Duration::from_secs(5),
                attempt.wait("echo", Duration::from_millis(200)),
            )
            .await
            .expect("the wait returns at its own deadline while the router is held");
            assert_eq!(answered, McpAttachmentReadiness::Pending);
            drop(held);

            // A tool held mid-dispatch keeps the router's read lock across
            // its server call: the wait's write acquisition is bounded too.
            let dispatching = adapter.router.read().await;
            let answered = tokio::time::timeout(
                Duration::from_secs(5),
                attempt.wait("echo", Duration::from_millis(200)),
            )
            .await
            .expect("the wait returns at its own deadline while a dispatch holds the router");
            assert_eq!(answered, McpAttachmentReadiness::Pending);
            drop(dispatching);
        }

        /// While a newer same-name attempt is still connecting, the older
        /// current connection serves the operation; it does not satisfy the
        /// newer attempt, which stays pending.
        #[cfg(feature = "workspace-tests")]
        #[tokio::test]
        async fn a_current_same_name_connection_does_not_satisfy_a_newer_attempt() {
            let adapter = Arc::new(McpRouterAdapter::new(generated_surface_router()));
            let first = stage(&adapter, &listing_server("same", &["echo"])).await;
            adapter.apply_staged().await.expect("apply first");
            let McpAttachmentReadiness::Usable {
                connection_generation,
            } = first.wait("echo", async_connect_test_timeout()).await
            else {
                panic!("the first attempt installs");
            };
            // Never answers the handshake: stays connecting.
            let second = stage(&adapter, &shell_server("same", "sleep 60")).await;
            adapter.apply_staged().await.expect("apply second");
            assert!(matches!(
                adapter.observe_operation("same", "echo").await,
                meerkat_core::McpOperationObservation::Callable { .. }
            ));
            assert_eq!(
                second.observe("echo").await,
                McpAttachmentReadiness::Pending
            );
            assert_eq!(
                second.wait("echo", Duration::from_millis(300)).await,
                McpAttachmentReadiness::Pending,
                "a deadline is a local not-ready answer"
            );
            assert_eq!(
                first.observe("echo").await,
                McpAttachmentReadiness::Usable {
                    connection_generation
                }
            );
            adapter.shutdown().await;
        }
    }
}
