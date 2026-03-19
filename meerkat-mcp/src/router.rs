//! MCP router for multi-server routing
//!
//! The router is a shell that manages connections, async tasks, and tool
//! caching. All lifecycle state transitions flow through the
//! [`ExternalToolSurfaceAuthority`] which is the single source of truth
//! for what state each surface is in and what transitions are legal.

use crate::external_tool_surface_authority::{
    ExternalToolSurfaceAuthority, ExternalToolSurfaceEffect, ExternalToolSurfaceInput,
    ExternalToolSurfaceMutator, SurfaceBaseState, SurfaceDeltaOperation, SurfaceDeltaPhase,
    SurfaceId, TurnNumber,
};
use crate::{McpConnection, McpError, McpServerConfig};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ContentBlock, ToolCallView, ToolResult};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const DEFAULT_REMOVAL_TIMEOUT: Duration = Duration::from_secs(30);
const PENDING_CHANNEL_CAPACITY: usize = 32;

/// MCP server lifecycle state used by staged router apply.
///
/// This is a public API projection of the authority's internal state.
/// The canonical truth lives in [`ExternalToolSurfaceAuthority`]; this
/// enum is derived from it for backward-compatible API consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerLifecycleState {
    Active,
    Removing {
        draining_since: Instant,
        timeout_at: Instant,
    },
    Removed,
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

pub type McpLifecyclePhase = ExternalToolDeltaPhase;
pub type McpLifecycleAction = ExternalToolDelta;

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
}

struct PendingResult {
    server_name: String,
    generation: u64,
    op: PendingOp,
    result: Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
}

struct CompletedLifecycleUpdate {
    action: McpLifecycleAction,
}

#[derive(Debug, Clone)]
enum StagedRouterOp {
    Add(McpServerConfig),
    Remove(String),
    Reload(McpReloadTarget),
}

/// Shell-level server entry. Holds connection, config, and tools.
/// Lifecycle state is owned by the authority, not by this struct.
struct ServerEntry {
    config: McpServerConfig,
    connection: Option<McpConnection>,
    tools: Vec<Arc<ToolDef>>,
    /// Shell-level atomic for inflight call RAII guards. The authority owns
    /// the canonical count; this atomic is for the `InflightCallGuard` pattern
    /// used by the `&self` call_tool path.
    active_calls: AtomicUsize,
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

/// Shell-level timeout tracking for a removing surface.
struct RemovalTimeout {
    draining_since: Instant,
    timeout_at: Instant,
}

/// Router for MCP tool calls across multiple servers.
///
/// All lifecycle state transitions flow through the
/// [`ExternalToolSurfaceAuthority`]. The router manages connections,
/// async tasks, tool caching, and effect execution.
///
/// Add and reload operations are non-blocking: `apply_staged()` spawns
/// background connection tasks and returns immediately. Completions are
/// delivered via [`take_external_updates`](Self::take_external_updates).
pub struct McpRouter {
    /// Canonical lifecycle authority — single source of truth.
    authority: ExternalToolSurfaceAuthority,

    servers: HashMap<String, ServerEntry>,
    /// Maps tool name to owning server, including servers in Removing state.
    tool_to_server: HashMap<String, String>,
    /// Visible tool cache from Active servers only.
    cached_tools: Vec<Arc<ToolDef>>,
    staged_ops: Vec<StagedRouterOp>,
    removal_timeout: Duration,
    /// Shell-level timeout tracking for Removing surfaces.
    removal_timeouts: HashMap<String, RemovalTimeout>,

    // --- Async pending infrastructure ---
    pending_tx: mpsc::Sender<PendingResult>,
    /// Poison-safe mutex wrapping the receiver.
    pending_rx: Mutex<mpsc::Receiver<PendingResult>>,
    pending_servers: HashMap<String, PendingState>,
    next_generation: u64,
    /// Queued canonical lifecycle deltas for async completions.
    completed_updates: VecDeque<CompletedLifecycleUpdate>,
}

impl McpRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        Self {
            authority: ExternalToolSurfaceAuthority::new(),
            servers: HashMap::new(),
            tool_to_server: HashMap::new(),
            cached_tools: Vec::new(),
            staged_ops: Vec::new(),
            removal_timeout: DEFAULT_REMOVAL_TIMEOUT,
            removal_timeouts: HashMap::new(),
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
    /// All state transitions flow through the authority. The shell spawns
    /// background tasks and executes effects.
    pub async fn apply_staged(&mut self) -> Result<McpApplyResult, McpError> {
        let mut delta = McpApplyDelta::default();
        let staged_ops = std::mem::take(&mut self.staged_ops);

        // 1. Drain pending results from background tasks.
        self.drain_pending();

        for op in staged_ops {
            match op {
                StagedRouterOp::Add(config) => {
                    let server_name = config.name.clone();
                    let sid = SurfaceId::from(server_name.as_str());

                    // Stage through authority.
                    if let Err(e) = self.authority.apply(ExternalToolSurfaceInput::StageAdd {
                        surface_id: sid.clone(),
                    }) {
                        tracing::warn!(server = %server_name, error = %e, "Authority rejected StageAdd");
                        continue;
                    }

                    // Cancel any existing pending op for this server.
                    self.pending_servers.remove(&server_name);

                    // Apply boundary through authority to move to Pending.
                    match self
                        .authority
                        .apply(ExternalToolSurfaceInput::ApplyBoundary {
                            surface_id: sid,
                            applied_at_turn: TurnNumber(0),
                        }) {
                        Ok(transition) => {
                            self.execute_effects(&transition.effects, &mut delta, Some(&config));
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %server_name,
                                error = %e,
                                "Authority rejected ApplyBoundary for Add"
                            );
                        }
                    }
                }
                StagedRouterOp::Remove(server_name) => {
                    let sid = SurfaceId::from(server_name.as_str());

                    // Cancel pending for same server if any.
                    self.pending_servers.remove(&server_name);

                    // Stage through authority.
                    if let Err(e) = self.authority.apply(ExternalToolSurfaceInput::StageRemove {
                        surface_id: sid.clone(),
                    }) {
                        tracing::warn!(server = %server_name, error = %e, "Authority rejected StageRemove");
                        continue;
                    }

                    // Apply boundary through authority.
                    match self
                        .authority
                        .apply(ExternalToolSurfaceInput::ApplyBoundary {
                            surface_id: sid,
                            applied_at_turn: TurnNumber(0),
                        }) {
                        Ok(transition) => {
                            self.execute_effects(&transition.effects, &mut delta, None);
                            // Record shell-level timeout if we entered Removing.
                            let check_sid = SurfaceId::from(server_name.as_str());
                            if self.authority.surface_base(&check_sid) == SurfaceBaseState::Removing
                            {
                                let draining_since = Instant::now();
                                self.removal_timeouts.insert(
                                    server_name,
                                    RemovalTimeout {
                                        draining_since,
                                        timeout_at: draining_since + self.removal_timeout,
                                    },
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %server_name,
                                error = %e,
                                "Authority rejected ApplyBoundary for Remove"
                            );
                        }
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
                    let sid = SurfaceId::from(server_name.as_str());

                    // Cancel any existing pending op for this server.
                    self.pending_servers.remove(&server_name);

                    // Stage through authority.
                    if let Err(e) = self.authority.apply(ExternalToolSurfaceInput::StageReload {
                        surface_id: sid.clone(),
                    }) {
                        tracing::warn!(server = %server_name, error = %e, "Authority rejected StageReload");
                        continue;
                    }

                    // Apply boundary through authority.
                    match self
                        .authority
                        .apply(ExternalToolSurfaceInput::ApplyBoundary {
                            surface_id: sid,
                            applied_at_turn: TurnNumber(0),
                        }) {
                        Ok(transition) => {
                            self.execute_effects(&transition.effects, &mut delta, Some(&config));
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %server_name,
                                error = %e,
                                "Authority rejected ApplyBoundary for Reload"
                            );
                        }
                    }
                }
            }
        }

        self.process_removals(&mut delta).await;
        self.rebuild_visible_cache();

        let pending_count = self.authority.pending_count();
        Ok(McpApplyResult {
            delta,
            pending_count,
        })
    }

    /// Execute effects returned by the authority.
    fn execute_effects(
        &mut self,
        effects: &[ExternalToolSurfaceEffect],
        delta: &mut McpApplyDelta,
        config: Option<&McpServerConfig>,
    ) {
        for effect in effects {
            match effect {
                ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                    surface_id: _,
                    operation,
                } => {
                    if let Some(config) = config {
                        let op = match operation {
                            SurfaceDeltaOperation::Add => PendingOp::Add,
                            SurfaceDeltaOperation::Reload => PendingOp::Reload,
                            _ => continue,
                        };
                        self.spawn_pending(config.clone(), op);
                    }
                }
                ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet => {
                    // Deferred: rebuild_visible_cache is called after all effects.
                }
                ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation,
                    phase,
                    ..
                } => {
                    let mcp_operation = match operation {
                        SurfaceDeltaOperation::Add => ToolConfigChangeOperation::Add,
                        SurfaceDeltaOperation::Remove => ToolConfigChangeOperation::Remove,
                        SurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
                        SurfaceDeltaOperation::None => continue,
                    };
                    let mcp_phase = match phase {
                        SurfaceDeltaPhase::Pending => McpLifecyclePhase::Pending,
                        SurfaceDeltaPhase::Applied => McpLifecyclePhase::Applied,
                        SurfaceDeltaPhase::Draining => McpLifecyclePhase::Draining,
                        SurfaceDeltaPhase::Failed => McpLifecyclePhase::Failed,
                        SurfaceDeltaPhase::Forced => McpLifecyclePhase::Forced,
                        SurfaceDeltaPhase::None => continue,
                    };
                    delta.lifecycle_actions.push(McpLifecycleAction::new(
                        surface_id.0.clone(),
                        mcp_operation,
                        mcp_phase,
                    ));
                }
                ExternalToolSurfaceEffect::CloseSurfaceConnection { surface_id } => {
                    if let Some(entry) = self.servers.get_mut(&surface_id.0) {
                        let conn = entry.connection.take();
                        let name = surface_id.0.clone();
                        tokio::spawn(async move {
                            Self::close_entry_connection(name, conn).await;
                        });
                        entry.tools.clear();
                    }
                    self.remove_tool_mappings_for_server(&surface_id.0);
                    delta.removed_servers.push(surface_id.0.clone());
                    self.removal_timeouts.remove(&surface_id.0);
                }
                ExternalToolSurfaceEffect::RejectSurfaceCall { .. } => {
                    // Rejections are handled inline during call_tool, not here.
                }
            }
        }
    }

    /// Spawn a background task to connect and enumerate tools for a server.
    fn spawn_pending(&mut self, config: McpServerConfig, op: PendingOp) {
        let generation = self.next_generation;
        self.next_generation += 1;

        let server_name = config.name.clone();
        self.pending_servers
            .insert(server_name.clone(), PendingState { generation });

        let tx = self.pending_tx.clone();
        tokio::spawn(async move {
            let result = McpConnection::connect_and_enumerate(&config).await;
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
        let results: Vec<PendingResult> = {
            let mut rx = match self.pending_rx.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!(
                        "MCP pending_rx mutex was poisoned; recovering. \
                         Router state may be inconsistent."
                    );
                    poisoned.into_inner()
                }
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

        self.pending_servers.remove(&result.server_name);
        let server_name = result.server_name.clone();
        let sid = SurfaceId::from(server_name.as_str());

        match result.result {
            Ok((conn, tools)) => {
                let tool_count = tools.len();

                // Notify authority of success.
                match self
                    .authority
                    .apply(ExternalToolSurfaceInput::PendingSucceeded {
                        surface_id: sid,
                        applied_at_turn: TurnNumber(0),
                    }) {
                    Ok(_transition) => {
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
                            tools,
                            active_calls: AtomicUsize::new(0),
                        };

                        if let Some(old_entry) = self.servers.insert(server_name.clone(), new_entry)
                            && result.op == PendingOp::Add
                        {
                            let old_name = old_entry.config.name.clone();
                            let old_conn = old_entry.connection;
                            tokio::spawn(async move {
                                Self::close_entry_connection(old_name, old_conn).await;
                            });
                        }

                        // Update tool mappings.
                        self.remove_tool_mappings_for_server(&server_name);
                        if let Some(entry) = self.servers.get(&server_name) {
                            for tool in &entry.tools {
                                self.tool_to_server
                                    .insert(tool.name.clone(), server_name.clone());
                            }
                        }

                        self.completed_updates.push_back(CompletedLifecycleUpdate {
                            action: McpLifecycleAction::new(
                                server_name,
                                operation,
                                McpLifecyclePhase::Applied,
                            )
                            .with_tool_count(Some(tool_count)),
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "Authority rejected PendingSucceeded"
                        );
                        let name = server_name;
                        tokio::spawn(async move {
                            if let Err(e) = conn.close().await {
                                tracing::debug!(
                                    "Error closing rejected MCP connection '{}': {}",
                                    name,
                                    e
                                );
                            }
                        });
                    }
                }
            }
            Err(err) => {
                let operation = match result.op {
                    PendingOp::Add => ToolConfigChangeOperation::Add,
                    PendingOp::Reload => ToolConfigChangeOperation::Reload,
                };

                // Notify authority of failure.
                if let Err(e) = self
                    .authority
                    .apply(ExternalToolSurfaceInput::PendingFailed {
                        surface_id: SurfaceId::from(server_name.as_str()),
                        applied_at_turn: TurnNumber(0),
                    })
                {
                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "Authority rejected PendingFailed"
                    );
                }

                tracing::warn!(
                    server = %server_name,
                    error = %err,
                    op = ?result.op,
                    "MCP server background connection failed"
                );

                self.completed_updates.push_back(CompletedLifecycleUpdate {
                    action: McpLifecycleAction::new(
                        server_name,
                        operation,
                        McpLifecyclePhase::Failed,
                    )
                    .with_detail(Some(err.to_string())),
                });
            }
        }
    }

    /// Drain pending results and return queued canonical lifecycle actions.
    pub fn take_lifecycle_actions(&mut self) -> Vec<McpLifecycleAction> {
        self.drain_pending();
        self.rebuild_visible_cache();
        self.completed_updates
            .drain(..)
            .map(|update| update.action)
            .collect()
    }

    /// Drain pending results and return queued external update notices.
    pub fn take_external_updates(&mut self) -> ExternalToolUpdate {
        self.drain_pending();
        self.rebuild_visible_cache();

        ExternalToolUpdate {
            notices: self
                .completed_updates
                .drain(..)
                .map(|update| update.action)
                .collect(),
            pending: self.pending_servers.keys().cloned().collect(),
        }
    }

    /// Returns true if there are pending background operations or undelivered notices.
    pub fn has_pending_or_notices(&self) -> bool {
        !self.pending_servers.is_empty() || !self.completed_updates.is_empty()
    }

    /// Backward-compatible immediate install path. Bypasses staged/boundary
    /// flow and directly drives the authority through Stage -> Apply -> Success.
    async fn install_active_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let (conn, tools) = McpConnection::connect_and_enumerate(&config).await?;

        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());

        // Drive authority: StageAdd -> ApplyBoundary -> PendingSucceeded.
        if let Err(e) = self.authority.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        }) {
            tracing::warn!(server = %server_name, error = %e, "Authority rejected StageAdd in install path");
        }
        if let Err(e) = self
            .authority
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn: TurnNumber(0),
            })
        {
            tracing::warn!(server = %server_name, error = %e, "Authority rejected ApplyBoundary in install path");
        }
        if let Err(e) = self
            .authority
            .apply(ExternalToolSurfaceInput::PendingSucceeded {
                surface_id: sid,
                applied_at_turn: TurnNumber(0),
            })
        {
            tracing::warn!(server = %server_name, error = %e, "Authority rejected PendingSucceeded in install path");
        }

        let new_entry = ServerEntry {
            config,
            connection: Some(conn),
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

    /// Process removal finalization based on shell-level timeout tracking.
    async fn process_removals(&mut self, delta: &mut McpApplyDelta) {
        let now = Instant::now();
        let mut finalized: Vec<(String, bool)> = Vec::new();

        let removing: Vec<SurfaceId> = self.authority.removing_surfaces().cloned().collect();
        for sid in &removing {
            let inflight = self.authority.inflight_call_count(sid);
            let timeout = self.removal_timeouts.get(&sid.0);

            if inflight == 0 {
                finalized.push((sid.0.clone(), false));
            } else if let Some(timeout) = timeout
                && now >= timeout.timeout_at
            {
                finalized.push((sid.0.clone(), true));
            }
        }

        for (server_name, degraded) in finalized {
            let sid = SurfaceId::from(server_name.as_str());
            let input = if degraded {
                ExternalToolSurfaceInput::FinalizeRemovalForced {
                    surface_id: sid,
                    applied_at_turn: TurnNumber(0),
                }
            } else {
                ExternalToolSurfaceInput::FinalizeRemovalClean {
                    surface_id: sid,
                    applied_at_turn: TurnNumber(0),
                }
            };

            match self.authority.apply(input) {
                Ok(transition) => {
                    self.execute_effects(&transition.effects, delta, None);
                    if degraded {
                        delta.degraded_removals.push(server_name);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "Authority rejected finalize removal"
                    );
                }
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
        let mut tools: Vec<Arc<ToolDef>> = Vec::new();
        for sid in self.authority.visible_surfaces() {
            if let Some(entry) = self.servers.get(&sid.0) {
                tools.extend(entry.tools.iter().cloned());
            }
        }
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        self.cached_tools = tools;
    }

    /// Get current lifecycle state for a server.
    ///
    /// Derives the lifecycle state from the authority's canonical state.
    pub fn server_lifecycle_state(&self, server_name: &str) -> Option<McpServerLifecycleState> {
        let sid = SurfaceId::from(server_name);
        let base = self.authority.surface_base(&sid);
        match base {
            SurfaceBaseState::Active => Some(McpServerLifecycleState::Active),
            SurfaceBaseState::Removing => {
                let timeout = self.removal_timeouts.get(server_name);
                match timeout {
                    Some(t) => Some(McpServerLifecycleState::Removing {
                        draining_since: t.draining_since,
                        timeout_at: t.timeout_at,
                    }),
                    None => Some(McpServerLifecycleState::Removing {
                        draining_since: Instant::now(),
                        timeout_at: Instant::now() + self.removal_timeout,
                    }),
                }
            }
            SurfaceBaseState::Removed => Some(McpServerLifecycleState::Removed),
            SurfaceBaseState::Absent => None,
        }
    }

    /// List names of all active (non-removed) servers.
    pub fn active_server_names(&self) -> Vec<String> {
        self.authority
            .visible_surfaces()
            .map(|sid| sid.0.clone())
            .collect()
    }

    /// Returns true if any server is currently in Removing state.
    pub fn has_removing_servers(&self) -> bool {
        self.authority.removing_surfaces().next().is_some()
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

    /// Call a tool by name, returning multimodal content blocks.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let server_name = self
            .tool_to_server
            .get(name)
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        let entry = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        // Check the authority for lifecycle state.
        let sid = SurfaceId::from(server_name.as_str());
        let base = self.authority.surface_base(&sid);
        if base != SurfaceBaseState::Active {
            return Err(McpError::ServerUnavailable {
                server: server_name.clone(),
                state: base.to_string(),
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
    pub async fn shutdown(mut self) {
        let _ = self.authority.apply(ExternalToolSurfaceInput::Shutdown);
        drop(self.pending_tx);
        for (_, entry) in self.servers {
            Self::close_entry_connection(entry.config.name, entry.connection).await;
        }
    }

    pub fn set_inflight_calls_for_testing(&mut self, server_name: &str, count: usize) {
        let sid = SurfaceId::from(server_name);
        if let Some(entry) = self.servers.get_mut(server_name) {
            let current = entry.active_calls.load(Ordering::Acquire);
            entry.active_calls.store(count, Ordering::Release);
            // Sync authority inflight count to match the shell's test override.
            if count > current {
                for _ in current..count {
                    let _ = self.authority.apply(ExternalToolSurfaceInput::CallStarted {
                        surface_id: sid.clone(),
                    });
                }
            } else {
                for _ in count..current {
                    let _ = self
                        .authority
                        .apply(ExternalToolSurfaceInput::CallFinished {
                            surface_id: sid.clone(),
                        });
                }
            }
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
        let blocks = self
            .call_tool(call.name, &args)
            .await
            .map_err(|e| match e {
                McpError::ToolNotFound(name) => ToolError::NotFound { name },
                other => ToolError::ExecutionFailed {
                    message: other.to_string(),
                },
            })?;

        Ok(ToolResult::with_blocks(call.id.to_string(), blocks, false))
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
    use crate::connection::McpConnection;
    use meerkat_core::event::ToolConfigChangeOperation;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn async_connect_test_timeout() -> Duration {
        Duration::from_secs((McpConnection::DEFAULT_CONNECT_TIMEOUT_SECS as u64) + 5)
    }

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

        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));

        router.stage_reload("test-server");
        let result = router.apply_staged().await.expect("apply staged reload");
        assert!(result.pending_count > 0 || !result.delta.reloaded_servers.is_empty());

        tokio::time::sleep(Duration::from_millis(500)).await;
        let ext = router.take_external_updates();
        assert!(
            ext.notices.iter().any(|n| n.target == "test-server")
                || router.server_lifecycle_state("test-server")
                    == Some(McpServerLifecycleState::Active),
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
            action.target == "test-server"
                && action.operation == ToolConfigChangeOperation::Remove
                && action.phase == McpLifecyclePhase::Forced
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

        assert!(
            result.pending_count > 0,
            "server should be pending after non-blocking add"
        );
        assert!(result.delta.lifecycle_actions.iter().any(|action| {
            action.target == "test-server"
                && action.operation == ToolConfigChangeOperation::Add
                && action.phase == McpLifecyclePhase::Pending
        }));

        let deadline = Instant::now() + async_connect_test_timeout();
        loop {
            let ext = router.take_external_updates();
            if ext
                .notices
                .iter()
                .any(|n| n.target == "test-server" && n.phase == McpLifecyclePhase::Applied)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for background MCP connect"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            !router.list_tools().is_empty(),
            "tools should be visible after activation"
        );
    }

    #[tokio::test]
    #[ignore = "exercises real subprocess timeout behavior"]
    async fn connect_and_enumerate_times_out() {
        let mut config = McpServerConfig::stdio(
            "hang-server",
            "sleep",
            vec!["60".to_string()],
            HashMap::new(),
        );
        config.connect_timeout_secs = Some(1);

        let result = McpConnection::connect_and_enumerate(&config).await;
        assert!(result.is_err(), "should time out");
        let err_msg = result.err().expect("already checked").to_string();
        assert!(
            err_msg.contains("Timed out"),
            "error should mention timeout: {err_msg}"
        );
    }

    #[tokio::test]
    async fn add_remove_add_discards_stale_generation() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();

        router.stage_add(test_server_config("test-server", &server_path));
        let result = router.apply_staged().await.expect("first add");
        assert_eq!(result.pending_count, 1);

        router.stage_remove("test-server");
        router.apply_staged().await.expect("remove");

        router.stage_add(test_server_config("test-server", &server_path));
        let result = router.apply_staged().await.expect("second add");
        assert_eq!(result.pending_count, 1);

        let deadline = Instant::now() + async_connect_test_timeout();
        loop {
            let ext = router.take_external_updates();
            if ext
                .notices
                .iter()
                .any(|n| n.target == "test-server" && n.phase == McpLifecyclePhase::Applied)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for second add to complete"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));
    }
}
