//! Adapter that bridges [`McpRouter`] to [`AgentToolDispatcher`].

use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::handles::McpServerLifecycleHandle;
use meerkat_core::{
    ExternalToolDelta, ExternalToolSurfaceSnapshot, ExternalToolUpdate, ToolCallView,
    ToolCatalogCapabilities, ToolCatalogEntry, ToolDef, ToolResult, agent::AgentToolDispatcher,
};
use serde_json::Value;
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
}

impl McpRouterAdapter {
    pub fn new(router: McpRouter) -> Self {
        let has_pending = router.has_pending_or_notices();
        let tools = AgentToolDispatcher::tools(&router);
        let catalog = AgentToolDispatcher::tool_catalog(&router);
        let pending_sources: Arc<[String]> = router.pending_sources_snapshot().into();
        let surface_snapshot = McpRouter::external_tool_surface_snapshot(&router);
        let mcp_lifecycle_handle = router.mcp_lifecycle_handle_slot();
        Self {
            router: AsyncRwLock::new(Some(router)),
            has_pending: AtomicBool::new(has_pending),
            tools_cache: StdRwLock::new(tools),
            catalog_cache: StdRwLock::new(catalog),
            pending_sources_cache: StdRwLock::new(pending_sources),
            surface_snapshot_cache: StdRwLock::new(Some(surface_snapshot)),
            mcp_lifecycle_handle,
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
        router.stage_add(config);
        self.sync_router_projection(router);
        Ok(())
    }

    /// Stage an MCP server remove operation.
    pub async fn stage_remove(&self, server_name: impl Into<String>) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router.stage_remove(server_name);
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
        router.stage_reload(target);
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

    /// Test helper for cross-crate runtime integration tests.
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::McpRouter;
    use crate::connection::McpConnection;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn async_connect_test_timeout() -> Duration {
        Duration::from_secs((McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS as u64) + 5)
    }

    fn test_server_path() -> PathBuf {
        if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
            return PathBuf::from(target_dir).join("debug/mcp-test-server");
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

    #[tokio::test]
    async fn empty_adapter_reports_exact_catalog_support() {
        let adapter = McpRouterAdapter::new(McpRouter::new());
        assert!(
            adapter.tool_catalog_capabilities().exact_catalog,
            "empty MCP adapters should preserve exact deferred catalogs on other tool planes"
        );
        assert!(adapter.tools().is_empty());
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
        router.stage_add(cfg);

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
        router.stage_add(test_server_config("srv-beta", &server_path));
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
        // Standalone router without a bound handle: every lifecycle seam
        // must still function (no panic, no fatal error).
        let mut router = McpRouter::new();
        let cfg = meerkat_core::McpServerConfig::stdio(
            "srv-standalone",
            "/bin/echo",
            Vec::<String>::new(),
            HashMap::new(),
        );
        router.stage_add(cfg);
        router.stage_reload("srv-standalone");
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
        router.stage_add(test_server_config("test-srv", &server_path));
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
        router.stage_add(test_server_config("planner", Path::new("/bin/echo")));

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
