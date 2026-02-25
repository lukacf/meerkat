//! MCP router for multi-server routing

use crate::{McpConnection, McpError, McpServerConfig};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ToolCallView, ToolResult};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_REMOVAL_TIMEOUT: Duration = Duration::from_secs(30);

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpLifecycleAction {
    Activated { server: String },
    RemovingStarted { server: String },
    Removed { server: String, degraded: bool },
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

/// Router for MCP tool calls across multiple servers
pub struct McpRouter {
    servers: HashMap<String, ServerEntry>,
    /// Maps tool name to owning server, including servers in Removing state.
    tool_to_server: HashMap<String, String>,
    /// Visible tool cache from Active servers only.
    cached_tools: Vec<Arc<ToolDef>>,
    staged_ops: Vec<StagedRouterOp>,
    removal_timeout: Duration,
}

impl McpRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            tool_to_server: HashMap::new(),
            cached_tools: Vec::new(),
            staged_ops: Vec::new(),
            removal_timeout: DEFAULT_REMOVAL_TIMEOUT,
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
        self.stage_add(config);
        self.apply_staged().await?;
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
        self.staged_ops.push(StagedRouterOp::Remove(server_name.into()));
    }

    /// Stage a server reload by server name (reuse existing config) or full config.
    pub fn stage_reload<T: Into<McpReloadTarget>>(&mut self, target: T) {
        self.staged_ops.push(StagedRouterOp::Reload(target.into()));
    }

    /// Apply staged operations at a boundary and return structural/lifecycle delta.
    pub async fn apply_staged(&mut self) -> Result<McpApplyDelta, McpError> {
        let mut delta = McpApplyDelta::default();
        let staged_ops = std::mem::take(&mut self.staged_ops);

        for op in staged_ops {
            match op {
                StagedRouterOp::Add(config) => {
                    let server_name = config.name.clone();
                    let existed = self
                        .servers
                        .get(&server_name)
                        .is_some_and(|entry| !matches!(entry.lifecycle, McpServerLifecycleState::Removed));
                    self.install_active_server(config).await?;

                    if existed {
                        delta.reloaded_servers.push(server_name.clone());
                        delta
                            .lifecycle_actions
                            .push(McpLifecycleAction::Reloaded { server: server_name });
                    } else {
                        delta.added_servers.push(server_name.clone());
                        delta
                            .lifecycle_actions
                            .push(McpLifecycleAction::Activated { server: server_name });
                    }
                }
                StagedRouterOp::Remove(server_name) => {
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
                    self.install_active_server(config).await?;
                    delta.reloaded_servers.push(server_name.clone());
                    delta
                        .lifecycle_actions
                        .push(McpLifecycleAction::Reloaded { server: server_name });
                }
            }
        }

        self.process_removals(&mut delta).await;
        self.rebuild_visible_cache();

        Ok(delta)
    }

    async fn install_active_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let conn = McpConnection::connect(&config).await?;
        let tools = conn
            .list_tools()
            .await?
            .into_iter()
            .map(Arc::new)
            .collect::<Vec<_>>();

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

    /// Gracefully shutdown all connections
    pub async fn shutdown(self) {
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
    use std::path::PathBuf;

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

    fn test_server_config(name: &str, path: &PathBuf) -> McpServerConfig {
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
        router.stage_add(test_server_config("test-server", &server_path));
        let delta = router.apply_staged().await.expect("apply staged add");

        assert_eq!(delta.added_servers, vec!["test-server"]);
        assert!(delta.removed_servers.is_empty());
        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));

        router.stage_reload("test-server");
        let delta = router.apply_staged().await.expect("apply staged reload");
        assert_eq!(delta.reloaded_servers, vec!["test-server"]);
        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));

        router.stage_remove("test-server");
        let delta = router.apply_staged().await.expect("apply staged remove");
        assert_eq!(delta.removed_servers, vec!["test-server"]);
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
        router.stage_add(test_server_config("test-server", &server_path));
        router.apply_staged().await.expect("apply add");

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
        router.stage_add(test_server_config("test-server", &server_path));
        router.apply_staged().await.expect("apply add");

        router.set_inflight_calls_for_testing("test-server", 1);
        router.stage_remove("test-server");
        let delta = router.apply_staged().await.expect("apply remove");

        assert!(delta.removed_servers.is_empty(), "should remain removing");
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
        let delta = router
            .apply_staged()
            .await
            .expect("apply should finalize drained remove");

        assert_eq!(delta.removed_servers, vec!["test-server"]);
        assert!(delta.degraded_removals.is_empty());
    }

    #[tokio::test]
    async fn removal_timeout_forces_close_and_reports_degraded_signal() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new_with_removal_timeout(Duration::from_millis(10));
        router.stage_add(test_server_config("test-server", &server_path));
        router.apply_staged().await.expect("apply add");

        router.set_inflight_calls_for_testing("test-server", 1);
        router.stage_remove("test-server");
        let delta = router.apply_staged().await.expect("apply remove start");
        assert!(delta.removed_servers.is_empty());

        tokio::time::sleep(Duration::from_millis(30)).await;
        let delta = router.apply_staged().await.expect("apply timeout finalize");

        assert_eq!(delta.removed_servers, vec!["test-server"]);
        assert_eq!(delta.degraded_removals, vec!["test-server"]);
        assert!(delta.lifecycle_actions.iter().any(|action| {
            matches!(
                action,
                McpLifecycleAction::Removed {
                    server,
                    degraded: true
                } if server == "test-server"
            )
        }));
    }
}
