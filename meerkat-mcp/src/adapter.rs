//! Adapter that bridges [`McpRouter`] to [`AgentToolDispatcher`].

use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceHandle, ExternalToolSurfaceInput,
    ExternalToolSurfaceTransition, McpServerLifecycleHandle, SurfaceDiagnosticSnapshot,
    SurfaceSnapshot,
};
use meerkat_core::{
    ExternalToolDelta, ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceFailureCause,
    ExternalToolSurfacePendingOp, ExternalToolSurfaceSnapshot, ExternalToolSurfaceStagedOp,
    ExternalToolUpdate, ToolCallView, ToolCatalogCapabilities, ToolCatalogEntry, ToolDef,
    ToolResult, agent::AgentToolDispatcher,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::RwLock as AsyncRwLock;

use crate::{McpApplyResult, McpReloadTarget, McpRouter};
use meerkat_core::McpServerConfig;

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
    /// Shared handle slot for the external-tool surface DSL owner. Standalone
    /// routers start with a local compatibility handle; runtime-backed session
    /// builds replace it with the session-owned handle through this slot.
    external_surface_handle: Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>>,
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
        Self {
            router: AsyncRwLock::new(Some(router)),
            has_pending: AtomicBool::new(has_pending),
            tools_cache: StdRwLock::new(tools),
            catalog_cache: StdRwLock::new(catalog),
            pending_sources_cache: StdRwLock::new(pending_sources),
            surface_snapshot_cache: StdRwLock::new(Some(surface_snapshot)),
            mcp_lifecycle_handle,
            external_surface_handle,
        }
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

    /// Gracefully shutdown the MCP router.
    ///
    /// Takes the router out of the adapter and shuts it down.
    /// After this call, tool calls will fail.
    pub async fn shutdown(&self) {
        let mut router = self.router.write().await;
        if let Some(router) = router.take() {
            router.shutdown().await;
        }
        self.has_pending.store(false, Ordering::Release);
        self.set_tools_cache(Arc::from([]));
        self.set_pending_sources_cache(Arc::from([]));
        self.set_surface_snapshot_cache(None);
        self.set_catalog_cache(Arc::from([]));
        self.set_pending_sources_cache(Arc::from([]));
        self.set_surface_snapshot_cache(None);
    }

    /// Stage an MCP server add operation.
    pub async fn stage_add(&self, config: McpServerConfig) -> Result<(), String> {
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

    /// Stage an MCP server reload operation.
    pub async fn stage_reload<T: Into<McpReloadTarget>>(&self, target: T) -> Result<(), String> {
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
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let result = router
            .apply_staged()
            .await
            .map_err(|error| error.to_string())?;
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
        self.sync_router_projection(router);
        Ok(actions)
    }

    /// Progress only Removing server finalization (drain/timeout) without applying staged ops.
    pub async fn progress_removals(&self) -> Result<crate::McpApplyDelta, String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let delta = router.progress_removals().await;
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

    /// Block until all pending MCP connections complete or timeout expires.
    ///
    /// Returns notices for completed/failed servers. Useful for CLI `--wait-for-mcp`
    /// and SDK `wait_for_mcp` workflows where the caller needs tools to be available
    /// before the first agent turn.
    pub async fn wait_until_ready(&self, timeout: std::time::Duration) -> Vec<ExternalToolDelta> {
        let mut all_notices = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let update = self.poll_external_updates().await;
            all_notices.extend(update.notices);
            if update.pending.is_empty() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    "wait_until_ready timed out after {}s with pending servers",
                    timeout.as_secs()
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        all_notices
    }

    /// Test helper for cross-crate lifecycle integration tests.
    pub async fn set_inflight_calls_for_testing(
        &self,
        server_name: &str,
        count: usize,
    ) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router.set_inflight_calls_for_testing(server_name, count);
        self.sync_router_projection(router);
        Ok(())
    }

    /// Test helper for controlling removal timeout from integration tests.
    pub async fn set_removal_timeout_for_testing(
        &self,
        removal_timeout: Duration,
    ) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router.set_removal_timeout(removal_timeout);
        self.sync_router_projection(router);
        Ok(())
    }
    fn transfer_pre_bind_external_surface_state(
        handle: &dyn ExternalToolSurfaceHandle,
        snapshot: &ExternalToolSurfaceSnapshot,
    ) -> Result<(), DslTransitionError> {
        let mut entries = snapshot.entries.clone();
        entries.sort_by(|left, right| {
            left.staged_intent_sequence
                .cmp(&right.staged_intent_sequence)
                .then_with(|| left.surface_id.cmp(&right.surface_id))
        });
        for entry in &entries {
            Self::transfer_pre_bind_surface_entry(handle, entry)?;
        }
        if snapshot.phase == meerkat_core::ExternalToolSurfaceGlobalPhase::Shutdown {
            handle.shutdown_surface()?;
        }
        Ok(())
    }

    fn transfer_pre_bind_surface_entry(
        handle: &dyn ExternalToolSurfaceHandle,
        entry: &ExternalToolSurfaceEntrySnapshot,
    ) -> Result<(), DslTransitionError> {
        match entry.pending_op {
            ExternalToolSurfacePendingOp::Add => {
                Self::replay_pending_surface_operation(
                    handle,
                    entry,
                    ExternalToolSurfaceDeltaOperation::Add,
                )?;
            }
            ExternalToolSurfacePendingOp::Reload => {
                Self::ensure_pre_bind_surface_active(handle, entry)?;
                Self::replay_pending_surface_operation(
                    handle,
                    entry,
                    ExternalToolSurfaceDeltaOperation::Reload,
                )?;
            }
            ExternalToolSurfacePendingOp::None => match entry.base_state {
                ExternalToolSurfaceBaseState::Active => {
                    Self::ensure_pre_bind_surface_active(handle, entry)?;
                }
                ExternalToolSurfaceBaseState::Removing => {
                    Self::ensure_pre_bind_surface_removing(handle, entry)?;
                }
                ExternalToolSurfaceBaseState::Absent | ExternalToolSurfaceBaseState::Removed => {}
            },
        }

        if entry.pending_op == ExternalToolSurfacePendingOp::None {
            Self::replay_pre_bind_staged_intent(handle, entry)?;
        }

        for _ in 0..entry.inflight_call_count {
            handle.call_started(entry.surface_id.clone())?;
        }

        Ok(())
    }

    fn ensure_pre_bind_surface_active(
        handle: &dyn ExternalToolSurfaceHandle,
        entry: &ExternalToolSurfaceEntrySnapshot,
    ) -> Result<(), DslTransitionError> {
        if matches!(
            handle
                .surface_snapshot(&entry.surface_id)
                .and_then(|snapshot| snapshot.base_state),
            Some(ExternalToolSurfaceBaseState::Active | ExternalToolSurfaceBaseState::Removing)
        ) {
            return Ok(());
        }

        handle.stage_add(entry.surface_id.clone(), 0)?;
        let staged_intent_sequence = Self::current_staged_sequence(handle, &entry.surface_id);
        handle.apply_boundary(
            entry.surface_id.clone(),
            0,
            staged_intent_sequence,
            staged_intent_sequence,
        )?;
        let pending_task_sequence = Self::current_pending_task_sequence(handle, &entry.surface_id);
        handle.mark_pending_succeeded(
            entry.surface_id.clone(),
            pending_task_sequence,
            staged_intent_sequence,
        )
    }

    fn ensure_pre_bind_surface_removing(
        handle: &dyn ExternalToolSurfaceHandle,
        entry: &ExternalToolSurfaceEntrySnapshot,
    ) -> Result<(), DslTransitionError> {
        if matches!(
            handle
                .surface_snapshot(&entry.surface_id)
                .and_then(|snapshot| snapshot.base_state),
            Some(ExternalToolSurfaceBaseState::Removing)
        ) {
            return Ok(());
        }
        Self::ensure_pre_bind_surface_active(handle, entry)?;
        handle.stage_remove(entry.surface_id.clone(), 0)?;
        let staged_intent_sequence = Self::current_staged_sequence(handle, &entry.surface_id);
        handle.apply_boundary(
            entry.surface_id.clone(),
            0,
            staged_intent_sequence,
            staged_intent_sequence,
        )
    }

    fn replay_pending_surface_operation(
        handle: &dyn ExternalToolSurfaceHandle,
        entry: &ExternalToolSurfaceEntrySnapshot,
        operation: ExternalToolSurfaceDeltaOperation,
    ) -> Result<(), DslTransitionError> {
        match operation {
            ExternalToolSurfaceDeltaOperation::Add => {
                handle.stage_add(entry.surface_id.clone(), 0)?;
            }
            ExternalToolSurfaceDeltaOperation::Reload => {
                handle.stage_reload(entry.surface_id.clone(), 0)?;
            }
            ExternalToolSurfaceDeltaOperation::Remove | ExternalToolSurfaceDeltaOperation::None => {
                return Ok(());
            }
        }
        let staged_intent_sequence = Self::current_staged_sequence(handle, &entry.surface_id);
        handle.apply_boundary(
            entry.surface_id.clone(),
            0,
            staged_intent_sequence,
            entry.staged_intent_sequence,
        )
    }

    fn replay_pre_bind_staged_intent(
        handle: &dyn ExternalToolSurfaceHandle,
        entry: &ExternalToolSurfaceEntrySnapshot,
    ) -> Result<(), DslTransitionError> {
        match entry.staged_op {
            ExternalToolSurfaceStagedOp::Add => handle.stage_add(entry.surface_id.clone(), 0),
            ExternalToolSurfaceStagedOp::Remove => {
                Self::ensure_pre_bind_surface_active(handle, entry)?;
                handle.stage_remove(entry.surface_id.clone(), 0)
            }
            ExternalToolSurfaceStagedOp::Reload => {
                Self::ensure_pre_bind_surface_active(handle, entry)?;
                handle.stage_reload(entry.surface_id.clone(), 0)
            }
            ExternalToolSurfaceStagedOp::None => Ok(()),
        }
    }

    fn current_staged_sequence(handle: &dyn ExternalToolSurfaceHandle, surface_id: &str) -> u64 {
        handle
            .surface_snapshot(surface_id)
            .and_then(|snapshot| snapshot.staged_intent_sequence)
            .unwrap_or(0)
    }

    fn current_pending_task_sequence(
        handle: &dyn ExternalToolSurfaceHandle,
        surface_id: &str,
    ) -> u64 {
        handle
            .surface_snapshot(surface_id)
            .and_then(|snapshot| snapshot.pending_task_sequence)
            .unwrap_or(0)
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouterAdapter {
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

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => {
                let args: Value = serde_json::from_str(call.args.get())
                    .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
                let blocks = router
                    .call_tool(call.name, &args)
                    .await
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                Ok(ToolResult::with_blocks(call.id.to_string(), blocks, false).into())
            }
            None => Err(ToolError::execution_failed("MCP router has been shut down")),
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

        self.sync_router_projection(router);
        update
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
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
        let pending_before_bind: Vec<String> =
            self.cached_pending_sources().iter().cloned().collect();
        for server_name in &pending_before_bind {
            if let Err(error) = handle.apply_connect_pending(server_name) {
                tracing::debug!(
                    server = %server_name,
                    error = %error,
                    "seed apply_connect_pending on bind rejected by DSL"
                );
            }
        }

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
        if let Some(snapshot) = self.cached_surface_snapshot()
            && let Err(error) =
                Self::transfer_pre_bind_external_surface_state(handle.as_ref(), &snapshot)
        {
            let error = error.to_string();
            tracing::warn!(
                error = %error,
                "failed to transfer pre-bind external surface state into session handle; poisoning surface owner"
            );
            poisoned_handle = Some(
                Arc::new(PoisonedExternalToolSurfaceHandle::new(error, snapshot))
                    as Arc<dyn ExternalToolSurfaceHandle>,
            );
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
        *guard = poisoned_handle.unwrap_or(handle);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::McpRouter;
    use crate::connection::McpConnection;
    use crate::router::CompatExternalToolSurfaceHandle;
    use meerkat_core::ExternalToolSurfacePendingOp;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn async_connect_test_timeout() -> Duration {
        Duration::from_secs((McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS as u64) + 5)
    }

    fn test_server_path() -> PathBuf {
        if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
            let target_dir = PathBuf::from(target_dir);
            for profile in ["debug", "release"] {
                let candidate = target_dir.join(profile).join("mcp-test-server");
                if candidate.exists() {
                    return candidate;
                }
            }
            return target_dir.join("debug/mcp-test-server");
        }

        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().expect("workspace root");
        workspace_root.join("target/debug/mcp-test-server")
    }

    fn skip_if_no_test_server() -> Option<PathBuf> {
        let path = test_server_path();
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "Skipping: mcp-test-server not built. \
                 Run `cargo build -p mcp-test-server` first."
            );
            None
        }
    }

    fn test_server_config(name: &str, path: &Path) -> meerkat_core::McpServerConfig {
        meerkat_core::McpServerConfig::stdio(
            name,
            path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
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

    #[tokio::test]
    async fn empty_adapter_reports_exact_catalog_support() {
        let adapter = McpRouterAdapter::new(McpRouter::new());
        assert!(
            adapter.tool_catalog_capabilities().exact_catalog,
            "empty MCP adapters should preserve exact deferred catalogs on other tool planes"
        );
        assert!(adapter.tools().is_empty());
    }

    #[tokio::test]
    async fn adapter_bind_external_surface_handle_replays_staged_router_state() {
        let mut router = McpRouter::new();
        router
            .stage_add(meerkat_core::McpServerConfig::stdio(
                "planner",
                "/bin/echo",
                Vec::<String>::new(),
                HashMap::new(),
            ))
            .expect("stage planner");
        let adapter = McpRouterAdapter::new(router);
        let handle: Arc<dyn ExternalToolSurfaceHandle> = Arc::new(
            CompatExternalToolSurfaceHandle::new(Duration::from_secs(30)),
        );

        adapter.bind_external_tool_surface_handle(Arc::clone(&handle));
        adapter.apply_staged().await.expect("apply staged");

        let snapshot = handle
            .surface_snapshot("planner")
            .expect("planner surface snapshot");
        assert_eq!(snapshot.pending_op, ExternalToolSurfacePendingOp::Add);
        assert_eq!(snapshot.pending_task_sequence, Some(1));
        assert_eq!(snapshot.pending_lineage_sequence, Some(1));
    }

    #[tokio::test]
    async fn adapter_bind_external_surface_handle_failure_poisons_existing_owner() {
        let mut router = McpRouter::new();
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

    #[tokio::test]
    async fn adapter_bind_external_surface_handle_replays_pending_router_state() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };
        let mut router = McpRouter::new();
        router
            .stage_add(test_server_config("late-bind-pending", &server_path))
            .expect("stage server");
        router
            .apply_staged()
            .await
            .expect("apply staged before bind");

        let adapter = McpRouterAdapter::new(router);
        let handle: Arc<dyn ExternalToolSurfaceHandle> = Arc::new(
            CompatExternalToolSurfaceHandle::new(Duration::from_secs(30)),
        );

        adapter.bind_external_tool_surface_handle(Arc::clone(&handle));
        adapter.wait_until_ready(async_connect_test_timeout()).await;

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
        fn pending_server_ids(&self) -> std::collections::BTreeSet<String> {
            std::collections::BTreeSet::new()
        }
    }

    #[tokio::test]
    async fn router_stage_add_fires_connect_pending_on_bound_lifecycle_handle() {
        let mut router = McpRouter::new();
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

    #[tokio::test]
    async fn adapter_bind_seeds_pre_bind_pending_servers_into_dsl() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };
        let mut router = McpRouter::new();
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

    #[tokio::test]
    async fn router_lifecycle_handle_none_is_noop() {
        // Router without a bound MCP lifecycle mirror: external surface
        // lifecycle still flows through its owner, while mirror notifications
        // remain no-ops.
        let handle: Arc<dyn ExternalToolSurfaceHandle> = Arc::new(
            CompatExternalToolSurfaceHandle::new(Duration::from_secs(30)),
        );
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
        let mut router = McpRouter::new_with_surface_handle(handle);
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
        let adapter = McpRouterAdapter::new(McpRouter::new());
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

    #[tokio::test]
    async fn wait_until_ready_returns_notices_when_server_connects() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();
        router
            .stage_add(test_server_config("test-srv", &server_path))
            .expect("stage add");
        router.apply_staged().await.expect("apply staged");

        let adapter = McpRouterAdapter::new(router);

        let notices = adapter.wait_until_ready(async_connect_test_timeout()).await;

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

    #[tokio::test]
    async fn connected_adapter_reports_exact_catalog_support_with_deferred_entries() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();
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

    #[tokio::test]
    async fn external_tool_surface_snapshot_reflects_staged_surface_state() {
        let mut router = McpRouter::new();
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
}
