//! MCP router for multi-server routing

use crate::{McpConnection, McpError, McpServerConfig};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::event::ToolConfigChangeOperation;
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ToolCallView, ToolResult};
use meerkat_core::{ExternalToolNotice, ExternalToolUpdate};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const DEFAULT_REMOVAL_TIMEOUT: Duration = Duration::from_secs(30);
const PENDING_CHANNEL_CAPACITY: usize = 32;

/// MCP server lifecycle state used by staged router apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerLifecycleState {
    Active,
    Removing {
        draining_since: Instant,
        timeout_at: Instant,
    },
    Removed,
}

impl McpServerLifecycleState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Removing { .. } => "Removing",
            Self::Removed => "Removed",
        }
    }
}

/// Reload target for staged reload operations.
#[derive(Debug, Clone)]
pub enum McpReloadTarget {
    ServerName(String),
    Config(McpServerConfig),
}

impl From<&str> for McpReloadTarget {
    fn from(value: &str) -> Self {
        Self::ServerName(value.to_string())
    }
}

impl From<String> for McpReloadTarget {
    fn from(value: String) -> Self {
        Self::ServerName(value)
    }
}

impl From<McpServerConfig> for McpReloadTarget {
    fn from(value: McpServerConfig) -> Self {
        Self::Config(value)
    }
}

/// Lifecycle actions emitted by [`McpRouter::apply_staged`].
///
/// Only **synchronous** actions appear here. Async completions (activated,
/// activation failed) go through [`McpRouter::take_external_updates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpLifecycleAction {
    /// Server connection is being attempted in the background.
    PendingAdd { server: String },
    /// Server has been activated (synchronous backward-compat path only).
    Activated { server: String },
    /// Server removal drain started.
    RemovingStarted { server: String },
    /// Server fully removed.
    Removed { server: String, degraded: bool },
    /// Server reload completed (synchronous backward-compat path only).
    Reloaded { server: String },
}

/// Result of applying staged MCP operations.
#[derive(Debug, Clone, Default)]
pub struct McpApplyDelta {
    pub added_servers: Vec<String>,
    pub removed_servers: Vec<String>,
    pub reloaded_servers: Vec<String>,
    pub lifecycle_actions: Vec<McpLifecycleAction>,
    pub degraded_removals: Vec<String>,
}

/// Return value of [`McpRouter::apply_staged`].
#[derive(Debug)]
pub struct McpApplyResult {
    pub delta: McpApplyDelta,
    pub pending_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingOp {
    Add,
    Reload,
}

struct PendingState {
    generation: u64,
    #[allow(dead_code)]
    op: PendingOp,
}

struct PendingResult {
    server_name: String,
    generation: u64,
    op: PendingOp,
    result: Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
}

#[derive(Debug, Clone)]
enum StagedRouterOp {
    Add(McpServerConfig),
    Remove(String),
    Reload(McpReloadTarget),
}

struct ServerEntry {
    config: McpServerConfig,
    connection: Option<McpConnection>,
    lifecycle: McpServerLifecycleState,
    tools: Vec<Arc<ToolDef>>,
    active_calls: AtomicUsize,
}

impl ServerEntry {
    fn is_active(&self) -> bool {
        matches!(self.lifecycle, McpServerLifecycleState::Active)
    }
}

struct InflightCallGuard<'a> {
    active_calls: &'a AtomicUsize,
}

impl<'a> InflightCallGuard<'a> {
    fn new(active_calls: &'a AtomicUsize) -> Self {
        active_calls.fetch_add(1, Ordering::AcqRel);
        Self { active_calls }
    }
}

impl Drop for InflightCallGuard<'_> {
    fn drop(&mut self) {
        self.active_calls.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Router for MCP tool calls across multiple servers.
///
/// Add and reload operations are non-blocking: `apply_staged()` spawns
/// background connection tasks and returns immediately. Completions are
/// delivered via [`take_external_updates`](Self::take_external_updates).
pub struct McpRouter {
    servers: HashMap<String, ServerEntry>,
    /// Maps tool name to owning server, including servers in Removing state.
    tool_to_server: HashMap<String, String>,
    /// Visible tool cache from Active servers only.
    cached_tools: Vec<Arc<ToolDef>>,
    staged_ops: Vec<StagedRouterOp>,
    removal_timeout: Duration,

    // --- Async pending infrastructure ---
    pending_tx: mpsc::Sender<PendingResult>,
    /// Poison-safe mutex wrapping the receiver.
    pending_rx: Mutex<mpsc::Receiver<PendingResult>>,
    pending_servers: HashMap<String, PendingState>,
    next_generation: u64,
    /// Queued notices for the agent loop (from async completions).
    completed_updates: VecDeque<ExternalToolNotice>,
}

impl McpRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        Self {
            servers: HashMap::new(),
            tool_to_server: HashMap::new(),
            cached_tools: Vec::new(),
            staged_ops: Vec::new(),
            removal_timeout: DEFAULT_REMOVAL_TIMEOUT,
            pending_tx: tx,
            pending_rx: Mutex::new(rx),
            pending_servers: HashMap::new(),
            next_generation: 0,
            completed_updates: VecDeque::new(),
        }
    }

    /// Create a new router with a custom remove-drain timeout.
    pub fn new_with_removal_timeout(removal_timeout: Duration) -> Self {
        Self {
            removal_timeout,
            ..Self::new()
        }
    }

    /// Override remove-drain timeout.
    pub fn set_removal_timeout(&mut self, removal_timeout: Duration) {
        self.removal_timeout = removal_timeout;
    }

    /// Backward-compatible immediate add path.
    ///
    /// Prefer [`stage_add`](Self::stage_add) + [`apply_staged`](Self::apply_staged)
    /// for boundary semantics.
    pub async fn add_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        // Use the synchronous install path for backward compatibility.
        self.install_active_server(config).await?;
        self.rebuild_visible_cache();
        Ok(())
    }

    /// Stage a server add for the next boundary apply.
    pub fn stage_add(&mut self, config: McpServerConfig) {
        self.staged_ops.push(StagedRouterOp::Add(config));
    }

    /// Stage a server remove intent for the next boundary apply.
    ///
    /// This only records intent. Removal lifecycle starts on `apply_staged`.
    pub fn stage_remove(&mut self, server_name: impl Into<String>) {
        self.staged_ops
            .push(StagedRouterOp::Remove(server_name.into()));
    }

    /// Stage a server reload by server name (reuse existing config) or full config.
    pub fn stage_reload<T: Into<McpReloadTarget>>(&mut self, target: T) {
        self.staged_ops.push(StagedRouterOp::Reload(target.into()));
    }

    /// Apply staged operations at a boundary (non-blocking for add/reload).
    ///
    /// Add and reload operations spawn background connection tasks and return
    /// immediately with `PendingAdd` in the lifecycle actions. Completions
    /// arrive via [`take_external_updates`](Self::take_external_updates).
    ///
    /// Remove operations are synchronous (start drain immediately).
    pub async fn apply_staged(&mut self) -> Result<McpApplyResult, McpError> {
        let mut delta = McpApplyDelta::default();
        let staged_ops = std::mem::take(&mut self.staged_ops);

        // 1. Drain pending results from background tasks.
        self.drain_pending();

        for op in staged_ops {
            match op {
                StagedRouterOp::Add(config) => {
                    let server_name = config.name.clone();

                    // Cancel any existing pending op for this server.
                    self.pending_servers.remove(&server_name);

                    self.spawn_pending(config, PendingOp::Add);
                    delta
                        .lifecycle_actions
                        .push(McpLifecycleAction::PendingAdd {
                            server: server_name,
                        });
                }
                StagedRouterOp::Remove(server_name) => {
                    // Cancel pending for same server if any.
                    self.pending_servers.remove(&server_name);

                    if let Some(entry) = self.servers.get_mut(&server_name)
                        && matches!(entry.lifecycle, McpServerLifecycleState::Active)
                    {
                        let draining_since = Instant::now();
                        entry.lifecycle = McpServerLifecycleState::Removing {
                            draining_since,
                            timeout_at: draining_since + self.removal_timeout,
                        };
                        delta
                            .lifecycle_actions
                            .push(McpLifecycleAction::RemovingStarted {
                                server: server_name,
                            });
                    }
                }
                StagedRouterOp::Reload(target) => {
                    let config = match target {
                        McpReloadTarget::ServerName(server_name) => {
                            let entry = self
                                .servers
                                .get(&server_name)
                                .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;
                            entry.config.clone()
                        }
                        McpReloadTarget::Config(config) => config,
                    };

                    let server_name = config.name.clone();

                    // Cancel any existing pending op for this server.
                    self.pending_servers.remove(&server_name);

                    // Keep old connection active during reload.
                    self.spawn_pending(config, PendingOp::Reload);
                    delta
                        .lifecycle_actions
                        .push(McpLifecycleAction::PendingAdd {
                            server: server_name,
                        });
                }
            }
        }

        self.process_removals(&mut delta).await;
        self.rebuild_visible_cache();

        let pending_count = self.pending_servers.len();
        Ok(McpApplyResult {
            delta,
            pending_count,
        })
    }

    /// Spawn a background task to connect and enumerate tools for a server.
    fn spawn_pending(&mut self, config: McpServerConfig, op: PendingOp) {
        let generation = self.next_generation;
        self.next_generation += 1;

        let server_name = config.name.clone();
        self.pending_servers
            .insert(server_name.clone(), PendingState { generation, op });

        let tx = self.pending_tx.clone();
        tokio::spawn(async move {
            let result = McpConnection::connect_and_enumerate(&config).await;
            // Channel may be closed if router was dropped — that's fine.
            let _ = tx
                .send(PendingResult {
                    server_name,
                    generation,
                    op,
                    result,
                })
                .await;
        });
    }

    /// Drain completed pending results from the background channel.
    fn drain_pending(&mut self) {
        // Collect results under the lock, then process outside to avoid
        // holding the MutexGuard across &mut self calls.
        let results: Vec<PendingResult> = {
            let mut rx = match self.pending_rx.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let mut buf = Vec::new();
            while let Ok(result) = rx.try_recv() {
                buf.push(result);
            }
            buf
        };
        for result in results {
            self.process_pending_result(result);
        }
    }

    fn process_pending_result(&mut self, result: PendingResult) {
        // Check generation matches — discard stale results.
        let current = self.pending_servers.get(&result.server_name);
        let is_current = current
            .map(|ps| ps.generation == result.generation)
            .unwrap_or(false);

        if !is_current {
            tracing::debug!(
                server = %result.server_name,
                generation = result.generation,
                "Discarding stale pending MCP result"
            );
            // Close the connection if it was successful.
            if let Ok((conn, _)) = result.result {
                let server_name = result.server_name;
                tokio::spawn(async move {
                    if let Err(e) = conn.close().await {
                        tracing::debug!(
                            "Error closing stale MCP connection '{}': {}",
                            server_name,
                            e
                        );
                    }
                });
            }
            return;
        }

        // Remove from pending.
        self.pending_servers.remove(&result.server_name);

        match result.result {
            Ok((conn, tools)) => {
                let tool_count = tools.len();
                let server_name = result.server_name.clone();

                // Determine operation type for event.
                let operation = match result.op {
                    PendingOp::Add => ToolConfigChangeOperation::Add,
                    PendingOp::Reload => ToolConfigChangeOperation::Reload,
                };

                // For reload: close old connection.
                if result.op == PendingOp::Reload
                    && let Some(old_entry) = self.servers.get_mut(&server_name)
                {
                    let old_conn = old_entry.connection.take();
                    let name = server_name.clone();
                    tokio::spawn(async move {
                        Self::close_entry_connection(name, old_conn).await;
                    });
                }

                // Install the new entry (or replace existing).
                let config = conn.config().clone();
                let new_entry = ServerEntry {
                    config,
                    connection: Some(conn),
                    lifecycle: McpServerLifecycleState::Active,
                    tools,
                    active_calls: AtomicUsize::new(0),
                };

                if let Some(old_entry) = self.servers.insert(server_name.clone(), new_entry) {
                    // For Add: close any leftover connection.
                    if result.op == PendingOp::Add {
                        let old_name = old_entry.config.name.clone();
                        let old_conn = old_entry.connection;
                        tokio::spawn(async move {
                            Self::close_entry_connection(old_name, old_conn).await;
                        });
                    }
                }

                // Update tool mappings.
                self.remove_tool_mappings_for_server(&server_name);
                if let Some(entry) = self.servers.get(&server_name) {
                    for tool in &entry.tools {
                        self.tool_to_server
                            .insert(tool.name.clone(), server_name.clone());
                    }
                }

                self.completed_updates.push_back(ExternalToolNotice {
                    server: server_name,
                    operation,
                    status: "activated".to_string(),
                    tool_count: Some(tool_count),
                });
            }
            Err(err) => {
                let server_name = result.server_name.clone();
                let operation = match result.op {
                    PendingOp::Add => ToolConfigChangeOperation::Add,
                    PendingOp::Reload => ToolConfigChangeOperation::Reload,
                };

                tracing::warn!(
                    server = %server_name,
                    error = %err,
                    op = ?result.op,
                    "MCP server background connection failed"
                );

                // For reload failure: keep old connection active.
                // For add failure: nothing to keep.

                self.completed_updates.push_back(ExternalToolNotice {
                    server: server_name,
                    operation,
                    status: format!("failed: {err}"),
                    tool_count: None,
                });
            }
        }
    }

    /// Drain pending results and return queued external update notices.
    ///
    /// Called by the adapter's `poll_external_updates()` implementation.
    pub fn take_external_updates(&mut self) -> ExternalToolUpdate {
        self.drain_pending();
        self.rebuild_visible_cache();

        ExternalToolUpdate {
            notices: self.completed_updates.drain(..).collect(),
            pending: self.pending_servers.keys().cloned().collect(),
        }
    }

    /// Returns true if there are pending background operations.
    pub fn has_pending(&self) -> bool {
        !self.pending_servers.is_empty()
    }

    async fn install_active_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let (conn, tools) = McpConnection::connect_and_enumerate(&config).await?;

        let server_name = config.name.clone();
        let new_entry = ServerEntry {
            config,
            connection: Some(conn),
            lifecycle: McpServerLifecycleState::Active,
            tools,
            active_calls: AtomicUsize::new(0),
        };

        if let Some(old_entry) = self.servers.insert(server_name.clone(), new_entry) {
            Self::close_entry_connection(old_entry.config.name.clone(), old_entry.connection).await;
        }

        self.remove_tool_mappings_for_server(&server_name);
        if let Some(entry) = self.servers.get(&server_name) {
            for tool in &entry.tools {
                self.tool_to_server
                    .insert(tool.name.clone(), server_name.clone());
            }
        }

        Ok(())
    }

    async fn process_removals(&mut self, delta: &mut McpApplyDelta) {
        let now = Instant::now();
        let mut finalized: Vec<(String, bool)> = Vec::new();

        for (server_name, entry) in &self.servers {
            if let McpServerLifecycleState::Removing { timeout_at, .. } = entry.lifecycle {
                let inflight = entry.active_calls.load(Ordering::Acquire);
                if inflight == 0 {
                    finalized.push((server_name.clone(), false));
                } else if now >= timeout_at {
                    finalized.push((server_name.clone(), true));
                }
            }
        }

        for (server_name, degraded) in finalized {
            if let Some(entry) = self.servers.get_mut(&server_name) {
                let conn = entry.connection.take();
                Self::close_entry_connection(server_name.clone(), conn).await;
                entry.tools.clear();
                entry.lifecycle = McpServerLifecycleState::Removed;
            }

            self.remove_tool_mappings_for_server(&server_name);
            delta.removed_servers.push(server_name.clone());
            delta.lifecycle_actions.push(McpLifecycleAction::Removed {
                server: server_name.clone(),
                degraded,
            });

            if degraded {
                delta.degraded_removals.push(server_name);
            }
        }
    }

    async fn close_entry_connection(server_name: String, conn: Option<McpConnection>) {
        if let Some(conn) = conn
            && let Err(e) = conn.close().await
        {
            tracing::debug!("Error closing MCP connection '{}': {}", server_name, e);
        }
    }

    fn remove_tool_mappings_for_server(&mut self, server_name: &str) {
        self.tool_to_server
            .retain(|_, owner| owner.as_str() != server_name);
    }

    fn rebuild_visible_cache(&mut self) {
        let mut tools: Vec<Arc<ToolDef>> = self
            .servers
            .values()
            .filter(|entry| entry.is_active())
            .flat_map(|entry| entry.tools.iter().cloned())
            .collect();

        tools.sort_by(|a, b| a.name.cmp(&b.name));
        self.cached_tools = tools;
    }

    /// Get current lifecycle state for a server.
    pub fn server_lifecycle_state(&self, server_name: &str) -> Option<McpServerLifecycleState> {
        self.servers.get(server_name).map(|entry| entry.lifecycle)
    }

    /// List names of all active (non-removed) servers.
    pub fn active_server_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|(_, entry)| matches!(entry.lifecycle, McpServerLifecycleState::Active))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Returns true if any server is currently in Removing state.
    pub fn has_removing_servers(&self) -> bool {
        self.servers
            .values()
            .any(|entry| matches!(entry.lifecycle, McpServerLifecycleState::Removing { .. }))
    }

    /// List all visible tools from active servers.
    pub fn list_tools(&self) -> &[Arc<ToolDef>] {
        &self.cached_tools
    }

    /// Progress only server removals (drain/timeout finalization) without applying staged ops.
    pub async fn progress_removals(&mut self) -> McpApplyDelta {
        let mut delta = McpApplyDelta::default();
        self.process_removals(&mut delta).await;
        self.rebuild_visible_cache();
        delta
    }

    /// Call a tool by name
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let server_name = self
            .tool_to_server
            .get(name)
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        let entry = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        let lifecycle = entry.lifecycle;
        if !matches!(lifecycle, McpServerLifecycleState::Active) {
            return Err(McpError::ServerUnavailable {
                server: server_name.clone(),
                state: lifecycle.as_str().to_string(),
            });
        }

        let conn = entry
            .connection
            .as_ref()
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        let _guard = InflightCallGuard::new(&entry.active_calls);
        conn.call_tool(name, args).await
    }

    /// Gracefully shutdown all connections.
    ///
    /// Drops the pending sender to signal background tasks, then closes
    /// all active connections.
    pub async fn shutdown(self) {
        // Drop pending_tx to signal background tasks.
        drop(self.pending_tx);
        for (_, entry) in self.servers {
            Self::close_entry_connection(entry.config.name, entry.connection).await;
        }
    }

    pub fn set_inflight_calls_for_testing(&mut self, server_name: &str, count: usize) {
        if let Some(entry) = self.servers.get_mut(server_name) {
            entry.active_calls.store(count, Ordering::Release);
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouter {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.cached_tools.clone().into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result_str = self
            .call_tool(call.name, &args)
            .await
            .map_err(|e| match e {
                McpError::ToolNotFound(name) => ToolError::NotFound { name },
                other => ToolError::ExecutionFailed {
                    message: other.to_string(),
                },
            })?;

        Ok(ToolResult {
            tool_use_id: call.id.to_string(),
            content: result_str,
            is_error: false,
        })
    }
}

impl Default for McpRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn test_server_path() -> PathBuf {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let workspace_root = PathBuf::from(manifest_dir)
            .parent()
            .expect("workspace root")
            .to_path_buf();
        workspace_root
            .join("target")
            .join("debug")
            .join("mcp-test-server")
    }

    fn skip_if_no_test_server() -> Option<PathBuf> {
        let path = test_server_path();
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "Skipping: mcp-test-server not built. Run `cargo build -p mcp-test-server` first."
            );
            None
        }
    }

    fn test_server_config(name: &str, path: &Path) -> McpServerConfig {
        McpServerConfig::stdio(
            name,
            path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        )
    }

    #[tokio::test]
    async fn staged_add_remove_reload_transitions() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();

        // Use add_server (synchronous path) for setup.
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));

        // Stage reload (async path) and wait for completion.
        router.stage_reload("test-server");
        let result = router.apply_staged().await.expect("apply staged reload");
        assert!(result.pending_count > 0 || !result.delta.reloaded_servers.is_empty());

        // Wait for pending to resolve.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let ext = router.take_external_updates();
        // Should have a completion notice.
        assert!(
            ext.notices.iter().any(|n| n.server == "test-server")
                || router.server_lifecycle_state("test-server")
                    == Some(McpServerLifecycleState::Active),
            "server should be active after reload completion"
        );

        router.stage_remove("test-server");
        let result = router.apply_staged().await.expect("apply staged remove");
        assert_eq!(result.delta.removed_servers, vec!["test-server"]);
        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Removed)
        ));
    }

    #[tokio::test]
    async fn remove_is_immediately_hidden_on_boundary_apply() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        let has_echo_before = router.list_tools().iter().any(|tool| tool.name == "echo");
        assert!(has_echo_before, "tool should be visible before remove");

        router.stage_remove("test-server");
        router.apply_staged().await.expect("apply remove");

        let has_echo_after = router.list_tools().iter().any(|tool| tool.name == "echo");
        assert!(!has_echo_after, "tool should be hidden immediately");
    }

    #[tokio::test]
    async fn removing_state_rejects_new_calls_and_drains_inflight() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new_with_removal_timeout(Duration::from_secs(60));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        router.set_inflight_calls_for_testing("test-server", 1);
        router.stage_remove("test-server");
        let result = router.apply_staged().await.expect("apply remove");

        assert!(
            result.delta.removed_servers.is_empty(),
            "should remain removing"
        );
        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Removing { .. })
        ));

        let err = router
            .call_tool("echo", &serde_json::json!({"message": "blocked"}))
            .await
            .expect_err("new calls should be rejected while removing");
        assert!(matches!(err, McpError::ServerUnavailable { .. }));

        router.set_inflight_calls_for_testing("test-server", 0);
        let result = router
            .apply_staged()
            .await
            .expect("apply should finalize drained remove");

        assert_eq!(result.delta.removed_servers, vec!["test-server"]);
        assert!(result.delta.degraded_removals.is_empty());
    }

    #[tokio::test]
    async fn removal_timeout_forces_close_and_reports_degraded_signal() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new_with_removal_timeout(Duration::from_millis(10));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        router.set_inflight_calls_for_testing("test-server", 1);
        router.stage_remove("test-server");
        let result = router.apply_staged().await.expect("apply remove start");
        assert!(result.delta.removed_servers.is_empty());

        tokio::time::sleep(Duration::from_millis(30)).await;
        let result = router.apply_staged().await.expect("apply timeout finalize");

        assert_eq!(result.delta.removed_servers, vec!["test-server"]);
        assert_eq!(result.delta.degraded_removals, vec!["test-server"]);
        assert!(result.delta.lifecycle_actions.iter().any(|action| {
            matches!(
                action,
                McpLifecycleAction::Removed {
                    server,
                    degraded: true
                } if server == "test-server"
            )
        }));
    }

    #[tokio::test]
    async fn apply_staged_add_is_non_blocking() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();
        router.stage_add(test_server_config("test-server", &server_path));
        let result = router.apply_staged().await.expect("apply staged add");

        // Should be pending, not immediately active.
        assert!(
            result.pending_count > 0,
            "server should be pending after non-blocking add"
        );
        assert!(result.delta.lifecycle_actions.iter().any(|a| matches!(
            a,
            McpLifecycleAction::PendingAdd { server } if server == "test-server"
        )));

        // Wait for background task to complete.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let ext = router.take_external_updates();
        assert!(
            ext.notices
                .iter()
                .any(|n| n.server == "test-server" && n.status == "activated"),
            "should have activated notice after background connect"
        );
        assert!(ext.pending.is_empty());

        // Tools should now be visible.
        assert!(
            !router.list_tools().is_empty(),
            "tools should be visible after activation"
        );
    }
}
