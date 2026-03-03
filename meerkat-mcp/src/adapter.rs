//! Adapter that bridges [`McpRouter`] to [`AgentToolDispatcher`].

use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::{
    ExternalToolUpdate, ToolCallView, ToolDef, ToolResult, agent::AgentToolDispatcher,
};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;

use crate::{McpApplyResult, McpReloadTarget, McpRouter, McpServerConfig};

/// Adapter that wraps an [`McpRouter`] to implement [`AgentToolDispatcher`].
///
/// Caches tools from the router for synchronous access via `tools()`.
/// Call [`refresh_tools()`](Self::refresh_tools) after router initialization
/// to populate the cache.
pub struct McpRouterAdapter {
    router: RwLock<Option<McpRouter>>,
    cached_tools: RwLock<Arc<[Arc<ToolDef>]>>,
    has_pending: AtomicBool,
}

impl McpRouterAdapter {
    pub fn new(router: McpRouter) -> Self {
        let has_pending = router.has_pending_or_notices();
        Self {
            router: RwLock::new(Some(router)),
            cached_tools: RwLock::new(Arc::from([])),
            has_pending: AtomicBool::new(has_pending),
        }
    }

    /// Refresh the cached tool list from the router.
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let router = self.router.read().await;
        if let Some(router) = router.as_ref() {
            let tools: Arc<[Arc<ToolDef>]> = router.list_tools().to_vec().into();
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
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
    }

    /// Stage an MCP server add operation.
    pub async fn stage_add(&self, config: McpServerConfig) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router.stage_add(config);
        Ok(())
    }

    /// Stage an MCP server remove operation.
    pub async fn stage_remove(&self, server_name: impl Into<String>) -> Result<(), String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        router.stage_remove(server_name);
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
        Ok(())
    }

    /// Apply staged MCP operations and refresh the visible tool cache.
    pub async fn apply_staged(&self) -> Result<McpApplyResult, String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let result = router
            .apply_staged()
            .await
            .map_err(|error| error.to_string())?;
        let tools: Arc<[Arc<ToolDef>]> = router.list_tools().to_vec().into();
        let mut cached = self.cached_tools.write().await;
        *cached = tools;
        self.has_pending.store(
            result.pending_count > 0 || router.has_pending_or_notices(),
            Ordering::Release,
        );
        Ok(result)
    }

    /// Progress only Removing server finalization (drain/timeout) without applying staged ops.
    pub async fn progress_removals(&self) -> Result<crate::McpApplyDelta, String> {
        let mut router = self.router.write().await;
        let router = router
            .as_mut()
            .ok_or_else(|| "MCP router has been shut down".to_string())?;
        let delta = router.progress_removals().await;
        let tools: Arc<[Arc<ToolDef>]> = router.list_tools().to_vec().into();
        let mut cached = self.cached_tools.write().await;
        *cached = tools;
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
        Ok(())
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouterAdapter {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self.cached_tools.try_read() {
            Ok(tools) => Arc::clone(&tools),
            Err(_) => Arc::from([]),
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => {
                let args: Value = serde_json::from_str(call.args.get())
                    .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
                let result = router
                    .call_tool(call.name, &args)
                    .await
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                Ok(ToolResult {
                    tool_use_id: call.id.to_string(),
                    content: result,
                    is_error: false,
                })
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

        // Refresh tool cache since new tools may have become visible.
        if !update.notices.is_empty() {
            let tools: Arc<[Arc<ToolDef>]> = router.list_tools().to_vec().into();
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
        }

        self.has_pending.store(
            !update.pending.is_empty() || router.has_pending_or_notices(),
            Ordering::Release,
        );
        update
    }
}
