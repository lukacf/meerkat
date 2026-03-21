//! MCP router for multi-server routing
//!
//! The router is a shell that manages connections, async tasks, and tool
//! caching. All lifecycle state transitions flow through the
//! [`ExternalToolSurfaceAuthority`] which is the single source of truth
//! for what state each surface is in and what transitions are legal.

use crate::external_tool_surface_authority::{
    ExternalToolSurfaceAuthority, ExternalToolSurfaceEffect, ExternalToolSurfaceInput,
    ExternalToolSurfaceMutator, PendingSurfaceOp, StagedSurfaceOp, SurfaceBaseState,
    SurfaceDeltaOperation, SurfaceDeltaPhase, SurfaceId, TurnNumber,
};
use crate::generated::{
    protocol_surface_completion::{self, SurfaceCompletionObligation},
    protocol_surface_snapshot_alignment::{self, SurfaceSnapshotAlignmentObligation},
};
use crate::{McpConnection, McpError};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ExternalToolUpdate;
use meerkat_core::McpServerConfig;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ContentBlock, ToolCallView, ToolResult};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, VecDeque};
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

struct PendingResult {
    obligation: SurfaceCompletionObligation,
    result: Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
}

struct CompletedLifecycleUpdate {
    action: McpLifecycleAction,
}

fn merge_snapshot_alignment(
    slot: &mut Option<SurfaceSnapshotAlignmentObligation>,
    candidate: SurfaceSnapshotAlignmentObligation,
) {
    match slot {
        Some(current) if current.snapshot_epoch >= candidate.snapshot_epoch => {}
        _ => *slot = Some(candidate),
    }
}

fn latest_snapshot_alignment(
    effects: &[ExternalToolSurfaceEffect],
) -> Option<SurfaceSnapshotAlignmentObligation> {
    let mut latest = None;
    for obligation in protocol_surface_snapshot_alignment::extract_obligations(effects) {
        merge_snapshot_alignment(&mut latest, obligation);
    }
    latest
}

#[derive(Debug, Clone)]
struct RouterProjectionSnapshot {
    epoch: u64,
    tool_to_server: HashMap<String, String>,
    visible_tools: Arc<[Arc<ToolDef>]>,
}

impl Default for RouterProjectionSnapshot {
    fn default() -> Self {
        Self {
            epoch: 0,
            tool_to_server: HashMap::new(),
            visible_tools: Arc::from([]),
        }
    }
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
    /// Wrapped in `Mutex` so that the `&self` `call_tool` path can apply
    /// `CallStarted`/`CallFinished` without requiring `&mut self`.
    authority: Mutex<ExternalToolSurfaceAuthority>,

    servers: HashMap<String, ServerEntry>,
    projection: Arc<RouterProjectionSnapshot>,
    staged_payloads: HashMap<String, McpServerConfig>,

    // --- Async pending infrastructure ---
    pending_tx: mpsc::Sender<PendingResult>,
    /// Poison-safe mutex wrapping the receiver.
    pending_rx: Mutex<mpsc::Receiver<PendingResult>>,
    pending_obligations: HashMap<String, SurfaceCompletionObligation>,
    pending_snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    /// Queued canonical lifecycle deltas for async completions.
    completed_updates: VecDeque<CompletedLifecycleUpdate>,
}

impl McpRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        Self {
            authority: Mutex::new(ExternalToolSurfaceAuthority::new()),
            servers: HashMap::new(),
            projection: Arc::new(RouterProjectionSnapshot::default()),
            staged_payloads: HashMap::new(),
            pending_tx: tx,
            pending_rx: Mutex::new(rx),
            pending_obligations: HashMap::new(),
            pending_snapshot_alignment: None,
            completed_updates: VecDeque::new(),
        }
    }

    /// Create a new router with a custom remove-drain timeout.
    pub fn new_with_removal_timeout(removal_timeout: Duration) -> Self {
        let (tx, rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        Self {
            authority: Mutex::new(ExternalToolSurfaceAuthority::with_removal_timeout(
                removal_timeout,
            )),
            servers: HashMap::new(),
            projection: Arc::new(RouterProjectionSnapshot::default()),
            staged_payloads: HashMap::new(),
            pending_tx: tx,
            pending_rx: Mutex::new(rx),
            pending_obligations: HashMap::new(),
            pending_snapshot_alignment: None,
            completed_updates: VecDeque::new(),
        }
    }

    /// Exclusive authority access (no lock contention — `&mut self` guarantees exclusivity).
    fn auth_mut(&mut self) -> &mut ExternalToolSurfaceAuthority {
        // SAFETY: get_mut only fails if the mutex is poisoned, which requires
        // a prior panic while holding the lock. Since &mut self guarantees
        // exclusivity and we never panic while holding, this is unreachable.
        #[allow(clippy::expect_used)]
        self.authority.get_mut().expect("authority mutex poisoned")
    }

    /// Shared authority access (acquires lock).
    fn auth(&self) -> std::sync::MutexGuard<'_, ExternalToolSurfaceAuthority> {
        #[allow(clippy::expect_used)]
        self.authority.lock().expect("authority mutex poisoned")
    }

    fn turn_number_from_staged_sequence(&self, staged_sequence: u64) -> TurnNumber {
        if staged_sequence > 0 {
            TurnNumber(staged_sequence)
        } else {
            // Defensive fallback: staged intents should always have non-zero sequence.
            TurnNumber(self.auth().snapshot_epoch())
        }
    }

    fn turn_number_from_snapshot_epoch(&self) -> TurnNumber {
        TurnNumber(self.auth().snapshot_epoch())
    }

    fn record_snapshot_alignment(
        &mut self,
        snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        if let Some(snapshot_alignment) = snapshot_alignment {
            merge_snapshot_alignment(&mut self.pending_snapshot_alignment, snapshot_alignment);
        }
    }

    /// Override remove-drain timeout.
    pub fn set_removal_timeout(&mut self, removal_timeout: Duration) {
        if let Err(error) = self.auth_mut().set_removal_timeout(removal_timeout) {
            tracing::warn!(
                timeout_ms = removal_timeout.as_millis(),
                error = %error,
                "Authority rejected set_removal_timeout"
            );
        }
    }

    /// Backward-compatible immediate add path.
    ///
    /// Prefer [`stage_add`](Self::stage_add) + [`apply_staged`](Self::apply_staged)
    /// for boundary semantics.
    pub async fn add_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        self.install_active_server(config).await?;
        let _ = self.publish_projection_snapshot();
        Ok(())
    }

    /// Stage a server add for the next boundary apply.
    pub fn stage_add(&mut self, config: McpServerConfig) {
        let server_name = config.name.clone();
        let surface_id = SurfaceId::from(server_name.as_str());
        match self
            .auth_mut()
            .apply(ExternalToolSurfaceInput::StageAdd { surface_id })
        {
            Ok(_) => {
                self.staged_payloads.insert(server_name, config);
            }
            Err(error) => {
                tracing::warn!(server = %server_name, error = %error, "Authority rejected StageAdd");
            }
        }
    }

    /// Stage a server remove intent for the next boundary apply.
    ///
    /// This only records intent. Removal lifecycle starts on `apply_staged`.
    pub fn stage_remove(&mut self, server_name: impl Into<String>) {
        let server_name = server_name.into();
        let surface_id = SurfaceId::from(server_name.as_str());
        if let Err(error) = self
            .auth_mut()
            .apply(ExternalToolSurfaceInput::StageRemove { surface_id })
        {
            tracing::warn!(server = %server_name, error = %error, "Authority rejected StageRemove");
            return;
        }
        self.staged_payloads.remove(&server_name);
    }

    /// Stage a server reload by server name (reuse existing config) or full config.
    pub fn stage_reload<T: Into<McpReloadTarget>>(&mut self, target: T) {
        let (server_name, requested_payload) = match target.into() {
            McpReloadTarget::ServerName(server_name) => (server_name, None),
            McpReloadTarget::Config(config) => (config.name.clone(), Some(config)),
        };
        let surface_id = SurfaceId::from(server_name.as_str());
        match self
            .auth_mut()
            .apply(ExternalToolSurfaceInput::StageReload { surface_id })
        {
            Ok(_) => {
                // Lifecycle legality is authority-owned. Shell payload lookup is
                // execution context only and does not decide whether StageReload
                // is legal.
                let payload = requested_payload.or_else(|| {
                    self.servers
                        .get(&server_name)
                        .map(|entry| entry.config.clone())
                });
                if let Some(payload) = payload {
                    self.staged_payloads.insert(server_name, payload);
                } else {
                    tracing::warn!(
                        server = %server_name,
                        "Authority accepted StageReload but no execution payload is available; apply_staged will surface this as a protocol error"
                    );
                    self.staged_payloads.remove(&server_name);
                }
            }
            Err(error) => {
                tracing::warn!(server = %server_name, error = %error, "Authority rejected StageReload");
            }
        }
    }

    /// Apply staged operations at a boundary (non-blocking for add/reload).
    ///
    /// All state transitions flow through the authority. The shell spawns
    /// background tasks and executes effects.
    pub async fn apply_staged(&mut self) -> Result<McpApplyResult, McpError> {
        let mut delta = McpApplyDelta::default();
        let staged_intents = self.auth_mut().staged_intents_in_order();
        let mut snapshot_alignment = None;

        // 1. Drain pending results from background tasks.
        self.drain_pending();

        for (surface_id, staged_op, staged_sequence) in staged_intents {
            let server_name = surface_id.0.clone();
            let config = match staged_op {
                StagedSurfaceOp::Add => Some(
                    self.staged_payloads
                        .get(&server_name)
                        .cloned()
                        .ok_or_else(|| McpError::ProtocolError {
                            message: format!(
                                "staged add for '{server_name}' is missing its staged payload"
                            ),
                        })?,
                ),
                StagedSurfaceOp::Reload => Some(
                    self.staged_payloads
                        .get(&server_name)
                        .cloned()
                        .ok_or_else(|| McpError::ProtocolError {
                            message: format!(
                                "staged reload for '{server_name}' is missing its staged payload"
                            ),
                        })?,
                ),
                StagedSurfaceOp::Remove | StagedSurfaceOp::None => None,
            };
            let applied_at_turn = self.turn_number_from_staged_sequence(staged_sequence);
            match self
                .auth_mut()
                .apply(ExternalToolSurfaceInput::ApplyBoundary {
                    surface_id,
                    applied_at_turn,
                }) {
                Ok(transition) => {
                    self.execute_effects(
                        &transition.effects,
                        &mut delta,
                        config.as_ref(),
                        &mut snapshot_alignment,
                    );
                    if matches!(staged_op, StagedSurfaceOp::Add | StagedSurfaceOp::Reload) {
                        self.staged_payloads.remove(&server_name);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %error,
                        "Authority rejected ApplyBoundary for staged intent"
                    );
                }
            }
        }

        self.process_removals(&mut delta, &mut snapshot_alignment)
            .await;
        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }

        let pending_count = self.auth_mut().pending_count();
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
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        // Extract obligation tokens from ScheduleSurfaceCompletion effects
        // before iterating — these are consumed by spawn_pending.
        let mut obligations = protocol_surface_completion::extract_obligations(effects)
            .into_iter()
            .collect::<VecDeque<_>>();
        let mut snapshot_obligations =
            protocol_surface_snapshot_alignment::extract_obligations(effects)
                .into_iter()
                .collect::<VecDeque<_>>();

        for effect in effects {
            match effect {
                ExternalToolSurfaceEffect::ScheduleSurfaceCompletion { operation, .. } => {
                    if let Some(config) = config {
                        if !matches!(
                            operation,
                            SurfaceDeltaOperation::Add | SurfaceDeltaOperation::Reload
                        ) {
                            continue;
                        }
                        if let Some(obligation) = obligations.pop_front() {
                            self.spawn_pending(config.clone(), obligation);
                        }
                    }
                }
                ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { .. } => {
                    if let Some(obligation) = snapshot_obligations.pop_front() {
                        merge_snapshot_alignment(snapshot_alignment, obligation);
                    }
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
                    delta.removed_servers.push(surface_id.0.clone());
                }
                ExternalToolSurfaceEffect::RejectSurfaceCall { .. } => {
                    // Rejections are handled inline during call_tool, not here.
                }
            }
        }
    }

    /// Spawn a background task to connect and enumerate tools for a server.
    fn spawn_pending(&mut self, config: McpServerConfig, obligation: SurfaceCompletionObligation) {
        let server_name = config.name.clone();
        self.pending_obligations
            .insert(server_name.clone(), obligation.clone());

        let tx = self.pending_tx.clone();
        tokio::spawn(async move {
            let result = McpConnection::connect_and_enumerate(&config).await;
            if let Err(error) = tx.send(PendingResult { obligation, result }).await {
                let server_name = error.0.obligation.surface_id.0.clone();
                McpRouter::close_result_connection_if_present(server_name, error.0.result);
            }
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
        let mut snapshot_alignment = None;
        for result in results {
            if let Some(obligation) = self.process_pending_result(result) {
                merge_snapshot_alignment(&mut snapshot_alignment, obligation);
            }
        }
        self.record_snapshot_alignment(snapshot_alignment);
        if self.pending_snapshot_alignment.is_some() {
            let published = self.publish_projection_snapshot();
            if published {
                let obligation = self.pending_snapshot_alignment.take();
                self.align_snapshot_if_requested(obligation);
            }
        }
    }

    fn process_pending_result(
        &mut self,
        result: PendingResult,
    ) -> Option<SurfaceSnapshotAlignmentObligation> {
        let PendingResult { obligation, result } = result;
        let server_name = obligation.surface_id.0.clone();
        let Some(state) = self.pending_obligations.get(&server_name) else {
            tracing::debug!(
                server = %server_name,
                "Discarding stale pending MCP result with no outstanding pending state"
            );
            Self::close_result_connection_if_present(server_name, result);
            return None;
        };

        let expected_pending_op = match obligation.operation {
            SurfaceDeltaOperation::Add => PendingSurfaceOp::Add,
            SurfaceDeltaOperation::Reload => PendingSurfaceOp::Reload,
            SurfaceDeltaOperation::Remove | SurfaceDeltaOperation::None => {
                Self::close_result_connection_if_present(server_name, result);
                return None;
            }
        };

        let obligation_matches = state.surface_id == obligation.surface_id
            && state.operation == obligation.operation
            && state.pending_task_sequence == obligation.pending_task_sequence
            && state.staged_intent_sequence == obligation.staged_intent_sequence
            && state.applied_at_turn == obligation.applied_at_turn;
        let sid = SurfaceId::from(server_name.as_str());
        let auth = self.auth();
        let authority_pending_matches = auth.pending_op(&sid) == expected_pending_op;
        let authority_task_matches =
            auth.pending_task_sequence(&sid) == obligation.pending_task_sequence;
        let authority_lineage_matches =
            auth.pending_lineage_sequence(&sid) == obligation.staged_intent_sequence;
        drop(auth);

        if !(obligation_matches
            && authority_pending_matches
            && authority_task_matches
            && authority_lineage_matches)
        {
            tracing::warn!(
                server = %server_name,
                operation = ?obligation.operation,
                obligation_matches,
                authority_pending_matches,
                authority_task_matches,
                authority_lineage_matches,
                "Discarding stale/inconsistent pending MCP result"
            );
            Self::close_result_connection_if_present(server_name.clone(), result);
            return None;
        }

        let Some(pending_state) = self.pending_obligations.remove(&server_name) else {
            tracing::warn!(
                server = %server_name,
                "Pending state disappeared while processing current pending MCP result"
            );
            Self::close_result_connection_if_present(server_name, result);
            return None;
        };

        match result {
            Ok((conn, tools)) => {
                let tool_count = tools.len();

                // Submit success feedback through the generated protocol helper,
                // consuming the obligation token from ScheduleSurfaceCompletion.
                match protocol_surface_completion::submit_pending_succeeded(
                    self.auth_mut(),
                    pending_state.clone(),
                ) {
                    Ok(transition) => {
                        let operation = match pending_state.operation {
                            SurfaceDeltaOperation::Add => ToolConfigChangeOperation::Add,
                            SurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
                            SurfaceDeltaOperation::Remove | SurfaceDeltaOperation::None => {
                                unreachable!()
                            }
                        };
                        let snapshot_alignment = latest_snapshot_alignment(&transition.effects);

                        // For reload: close old connection.
                        if pending_state.operation == SurfaceDeltaOperation::Reload
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
                            && pending_state.operation == SurfaceDeltaOperation::Add
                        {
                            let old_name = old_entry.config.name.clone();
                            let old_conn = old_entry.connection;
                            tokio::spawn(async move {
                                Self::close_entry_connection(old_name, old_conn).await;
                            });
                        }

                        self.completed_updates.push_back(CompletedLifecycleUpdate {
                            action: McpLifecycleAction::new(
                                server_name,
                                operation,
                                McpLifecyclePhase::Applied,
                            )
                            .with_tool_count(Some(tool_count)),
                        });
                        return snapshot_alignment;
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
                        return None;
                    }
                }
            }
            Err(err) => {
                let operation = match pending_state.operation {
                    SurfaceDeltaOperation::Add => ToolConfigChangeOperation::Add,
                    SurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
                    SurfaceDeltaOperation::Remove | SurfaceDeltaOperation::None => unreachable!(),
                };

                // Submit failure feedback through the generated protocol helper,
                // consuming the obligation token from ScheduleSurfaceCompletion.
                let snapshot_alignment = match protocol_surface_completion::submit_pending_failed(
                    self.auth_mut(),
                    pending_state.clone(),
                ) {
                    Ok(transition) => latest_snapshot_alignment(&transition.effects),
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "Authority rejected PendingFailed"
                        );
                        None
                    }
                };

                tracing::warn!(
                    server = %server_name,
                    error = %err,
                    op = ?pending_state.operation,
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
                return snapshot_alignment;
            }
        }
    }

    /// Drain pending results and return queued canonical lifecycle actions.
    pub fn take_lifecycle_actions(&mut self) -> Vec<McpLifecycleAction> {
        self.drain_pending();
        self.completed_updates
            .drain(..)
            .map(|update| update.action)
            .collect()
    }

    /// Drain pending results and return queued external update notices.
    pub fn take_external_updates(&mut self) -> ExternalToolUpdate {
        self.drain_pending();
        let pending = self
            .auth()
            .pending_surfaces()
            .map(|surface_id| surface_id.0.clone())
            .collect();

        ExternalToolUpdate {
            notices: self
                .completed_updates
                .drain(..)
                .map(|update| update.action)
                .collect(),
            pending,
        }
    }

    /// Returns true if there are pending background operations or undelivered notices.
    pub fn has_pending_or_notices(&self) -> bool {
        self.auth().pending_count() > 0 || !self.completed_updates.is_empty()
    }

    /// Backward-compatible immediate install path. Bypasses staged/boundary
    /// flow and directly drives the authority through Stage -> Apply -> Success.
    async fn install_active_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let (conn, tools) = McpConnection::connect_and_enumerate(&config).await?;

        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());
        let mut snapshot_alignment = None;

        // Drive authority: StageAdd -> ApplyBoundary -> PendingSucceeded.
        // Uses the generated protocol helper for the completion feedback.
        if let Err(e) = self.auth_mut().apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        }) {
            tracing::warn!(server = %server_name, error = %e, "Authority rejected StageAdd in install path");
        }
        let staged_sequence = self.auth().staged_intent_sequence(&sid);
        let applied_at_turn = self.turn_number_from_staged_sequence(staged_sequence);
        let boundary_effects = match self.auth_mut().apply(
            ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn,
            },
        ) {
            Ok(t) => t.effects,
            Err(e) => {
                tracing::warn!(server = %server_name, error = %e, "Authority rejected ApplyBoundary in install path");
                vec![]
            }
        };
        // Extract and immediately consume the obligation for the synchronous install path.
        let obligations = protocol_surface_completion::extract_obligations(&boundary_effects);
        for obligation in obligations {
            match protocol_surface_completion::submit_pending_succeeded(self.auth_mut(), obligation)
            {
                Ok(transition) => {
                    if let Some(obligation) = latest_snapshot_alignment(&transition.effects) {
                        merge_snapshot_alignment(&mut snapshot_alignment, obligation);
                    }
                }
                Err(e) => {
                    tracing::warn!(server = %server_name, error = %e, "Authority rejected PendingSucceeded in install path");
                }
            }
        }

        let new_entry = ServerEntry {
            config,
            connection: Some(conn),
            tools,
            active_calls: AtomicUsize::new(0),
        };
        let tool_count = new_entry.tools.len();

        if let Some(old_entry) = self.servers.insert(server_name.clone(), new_entry) {
            Self::close_entry_connection(old_entry.config.name.clone(), old_entry.connection).await;
        }

        self.completed_updates.push_back(CompletedLifecycleUpdate {
            action: McpLifecycleAction::new(
                server_name.clone(),
                ToolConfigChangeOperation::Add,
                McpLifecyclePhase::Applied,
            )
            .with_tool_count(Some(tool_count)),
        });

        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }

        Ok(())
    }

    /// Process removal finalization based on authority-owned timeout tracking.
    async fn process_removals(
        &mut self,
        delta: &mut McpApplyDelta,
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        let now = Instant::now();
        let mut finalized: Vec<(String, bool)> = Vec::new();

        let removing: Vec<SurfaceId> = self.auth_mut().removing_surfaces().cloned().collect();
        for sid in &removing {
            let inflight = self.auth_mut().inflight_call_count(sid);
            let timing = self.auth_mut().removal_timing(sid);

            if inflight == 0 {
                finalized.push((sid.0.clone(), false));
            } else if let Some(timing) = timing
                && now >= timing.timeout_at
            {
                finalized.push((sid.0.clone(), true));
            }
        }

        for (server_name, degraded) in finalized {
            let sid = SurfaceId::from(server_name.as_str());
            let applied_at_turn = match self.auth_mut().removal_timing(&sid) {
                Some(timing) => timing.applied_at_turn,
                None => {
                    tracing::warn!(
                        server = %server_name,
                        "removal finalization missing authority timing; falling back to snapshot epoch"
                    );
                    self.turn_number_from_snapshot_epoch()
                }
            };
            let input = if degraded {
                ExternalToolSurfaceInput::FinalizeRemovalForced {
                    surface_id: sid,
                    applied_at_turn,
                }
            } else {
                ExternalToolSurfaceInput::FinalizeRemovalClean {
                    surface_id: sid,
                    applied_at_turn,
                }
            };

            match self.auth_mut().apply(input) {
                Ok(transition) => {
                    self.execute_effects(&transition.effects, delta, None, snapshot_alignment);
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

    fn align_snapshot_if_requested(
        &mut self,
        snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        let Some(obligation) = snapshot_alignment else {
            return;
        };
        if let Err(error) = protocol_surface_snapshot_alignment::submit_snapshot_aligned(
            self.auth_mut(),
            obligation.clone(),
        ) {
            tracing::warn!(
                snapshot_epoch = obligation.snapshot_epoch,
                error = %error,
                "Authority rejected SnapshotAligned"
            );
        }
    }

    async fn close_entry_connection(server_name: String, conn: Option<McpConnection>) {
        if let Some(conn) = conn
            && let Err(e) = conn.close().await
        {
            tracing::debug!("Error closing MCP connection '{}': {}", server_name, e);
        }
    }

    fn close_result_connection_if_present(
        server_name: String,
        result: Result<(McpConnection, Vec<Arc<ToolDef>>), McpError>,
    ) {
        if let Ok((conn, _)) = result {
            tokio::spawn(async move {
                if let Err(error) = conn.close().await {
                    tracing::debug!(
                        "Error closing stale MCP connection '{}': {}",
                        server_name,
                        error
                    );
                }
            });
        }
    }

    fn publish_projection_snapshot(&mut self) -> bool {
        let auth = self.auth();
        let epoch = auth.snapshot_epoch();
        let server_names: BTreeSet<String> =
            auth.visible_surfaces().map(|sid| sid.0.clone()).collect();
        drop(auth);

        let mut tool_to_server = HashMap::new();
        let mut visible_tools: Vec<Arc<ToolDef>> = Vec::new();
        let mut complete = true;

        for server_name in server_names {
            let Some(entry) = self.servers.get(&server_name) else {
                tracing::warn!(
                    server = %server_name,
                    "Visible server has no shell entry during projection publish; publishing conservative snapshot without this server"
                );
                complete = false;
                continue;
            };
            for tool in &entry.tools {
                if let Some(previous_owner) =
                    tool_to_server.insert(tool.name.clone(), server_name.clone())
                    && previous_owner != server_name
                {
                    tracing::warn!(
                        tool = %tool.name,
                        previous_owner = %previous_owner,
                        current_owner = %server_name,
                        "MCP projection remapped duplicate tool name to newer owner"
                    );
                }
                visible_tools.push(Arc::clone(tool));
            }
        }
        visible_tools.sort_by(|a, b| a.name.cmp(&b.name));

        self.projection = Arc::new(RouterProjectionSnapshot {
            epoch,
            tool_to_server,
            visible_tools: visible_tools.into(),
        });
        complete
    }

    fn projection_tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.projection.visible_tools)
    }

    /// Get current lifecycle state for a server.
    ///
    /// Derives the lifecycle state from the authority's canonical state.
    pub fn server_lifecycle_state(&self, server_name: &str) -> Option<McpServerLifecycleState> {
        let sid = SurfaceId::from(server_name);
        let auth = self.auth();
        let base = auth.surface_base(&sid);
        match base {
            SurfaceBaseState::Active => Some(McpServerLifecycleState::Active),
            SurfaceBaseState::Removing => {
                let timing = auth.removal_timing(&sid);
                match timing {
                    Some(t) => Some(McpServerLifecycleState::Removing {
                        draining_since: t.draining_since,
                        timeout_at: t.timeout_at,
                    }),
                    None => {
                        // Defensive fallback: authority should always have timing
                        // for Removing surfaces, but synthesize if missing.
                        let now = Instant::now();
                        Some(McpServerLifecycleState::Removing {
                            draining_since: now,
                            timeout_at: now + DEFAULT_REMOVAL_TIMEOUT,
                        })
                    }
                }
            }
            SurfaceBaseState::Removed => Some(McpServerLifecycleState::Removed),
            SurfaceBaseState::Absent => None,
        }
    }

    /// List names of all active (non-removed) servers.
    pub fn active_server_names(&self) -> Vec<String> {
        self.auth()
            .visible_surfaces()
            .map(|sid| sid.0.clone())
            .collect()
    }

    /// Returns true if any server is currently in Removing state.
    pub fn has_removing_servers(&self) -> bool {
        self.auth().removing_surfaces().next().is_some()
    }

    /// List all visible tools from active servers.
    pub fn list_tools(&self) -> &[Arc<ToolDef>] {
        self.projection.visible_tools.as_ref()
    }

    /// Progress only server removals (drain/timeout finalization) without applying staged ops.
    pub async fn progress_removals(&mut self) -> McpApplyDelta {
        let mut delta = McpApplyDelta::default();
        let mut snapshot_alignment = None;
        self.process_removals(&mut delta, &mut snapshot_alignment)
            .await;
        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }
        delta
    }

    /// Call a tool by name, returning multimodal content blocks.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let snapshot = Arc::clone(&self.projection);
        let _projection_epoch = snapshot.epoch;
        let server_name = snapshot
            .tool_to_server
            .get(name)
            .cloned()
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        let entry = self
            .servers
            .get(&server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        // Route through authority — CallStarted validates lifecycle state.
        let sid = SurfaceId::from(server_name.as_str());
        {
            let mut auth = self.auth();
            match auth.apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            }) {
                Ok(transition) => {
                    // Check for RejectSurfaceCall effect.
                    for effect in &transition.effects {
                        if let ExternalToolSurfaceEffect::RejectSurfaceCall { reason, .. } = effect
                        {
                            return Err(McpError::ServerUnavailable {
                                server: server_name.clone(),
                                state: reason.clone(),
                            });
                        }
                    }
                }
                Err(e) => {
                    return Err(McpError::ServerUnavailable {
                        server: server_name.clone(),
                        state: e.to_string(),
                    });
                }
            }
        }

        let conn = entry
            .connection
            .as_ref()
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        let _guard = InflightCallGuard::new(&entry.active_calls);
        let result = conn.call_tool(name, args).await;

        // Notify authority that the call finished.
        let mut auth = self.auth();
        let _ = auth.apply(ExternalToolSurfaceInput::CallFinished { surface_id: sid });

        result
    }

    /// Gracefully shutdown all connections.
    pub async fn shutdown(mut self) {
        // First consume any finished pending tasks through normal processing so
        // stale completion payloads close their transports via existing paths.
        self.drain_pending();
        let _ = self.auth_mut().apply(ExternalToolSurfaceInput::Shutdown);
        self.pending_obligations.clear();
        self.pending_snapshot_alignment = None;
        self.completed_updates.clear();
        self.staged_payloads.clear();
        let (replacement_tx, _replacement_rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        let old_pending_tx = std::mem::replace(&mut self.pending_tx, replacement_tx);
        drop(old_pending_tx);
        // Drain any completion payloads that arrived after pending_tx drop.
        let drained_results: Vec<PendingResult> = {
            let mut rx = match self.pending_rx.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!(
                        "MCP pending_rx mutex was poisoned during shutdown; recovering receiver"
                    );
                    poisoned.into_inner()
                }
            };
            let mut drained = Vec::new();
            while let Ok(result) = rx.try_recv() {
                drained.push(result);
            }
            drained
        };
        for pending in drained_results {
            if let Ok((conn, _)) = pending.result {
                Self::close_entry_connection(pending.obligation.surface_id.0, Some(conn)).await;
            }
        }
        let servers = std::mem::take(&mut self.servers);
        for (_, entry) in servers {
            Self::close_entry_connection(entry.config.name, entry.connection).await;
        }
        let _ = self.publish_projection_snapshot();
    }

    pub fn set_inflight_calls_for_testing(&mut self, server_name: &str, count: usize) {
        let sid = SurfaceId::from(server_name);
        if let Some(entry) = self.servers.get_mut(server_name) {
            let current = entry.active_calls.load(Ordering::Acquire);
            entry.active_calls.store(count, Ordering::Release);
            // Sync authority inflight count to match the shell's test override.
            if count > current {
                for _ in current..count {
                    let _ = self
                        .auth_mut()
                        .apply(ExternalToolSurfaceInput::CallStarted {
                            surface_id: sid.clone(),
                        });
                }
            } else {
                for _ in count..current {
                    let _ = self
                        .auth_mut()
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
        self.projection_tools()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
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

        Ok(ToolResult::with_blocks(call.id.to_string(), blocks, false).into())
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
        if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
            return PathBuf::from(target_dir).join("debug/mcp-test-server");
        }

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
        assert!(
            matches!(err, McpError::ToolNotFound(_)),
            "removing surfaces should be absent from the published routing snapshot, got {err:?}"
        );

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
