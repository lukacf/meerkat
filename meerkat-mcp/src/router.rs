//! MCP router for multi-server routing
//!
//! The router is a shell that manages connections, async tasks, and tool
//! caching. Runtime/session construction injects a generated machine-owned
//! [`ExternalToolSurfaceHandle`] as the surface owner. Construction without a
//! bound handle fails closed for lifecycle mutations until that owner is
//! supplied.

use crate::external_tool_surface_authority::{
    ExternalToolSurfaceEffect, ExternalToolSurfaceError, ExternalToolSurfaceInput,
    ExternalToolSurfacePhase, ExternalToolSurfaceTransition, RemovalTimingInfo, StagedSurfaceOp,
    SurfaceBaseState, SurfaceDeltaOperation, SurfaceDeltaPhase, SurfaceId, TurnNumber,
};
use crate::generated::{
    protocol_surface_completion::{self, SurfaceCompletionObligation},
    protocol_surface_snapshot_alignment::{self, SurfaceSnapshotAlignmentObligation},
};
use crate::{McpAuthResolver, McpConnection, McpError};
use async_trait::async_trait;
use meerkat_auth_core::McpAuthMode;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ExternalToolUpdate;
use meerkat_core::McpServerConfig;
use meerkat_core::ToolCatalogCapabilities;
use meerkat_core::ToolCatalogEntry;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceEffect as CoreSurfaceEffect, ExternalToolSurfaceHandle,
    ExternalToolSurfaceInput as CoreSurfaceInput,
    ExternalToolSurfaceTransition as CoreSurfaceTransition, McpServerLifecycleHandle,
    SurfaceDiagnosticSnapshot, SurfaceSnapshot,
};
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ContentBlock, ToolCallView, ToolResult};
use meerkat_core::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceFailureCause,
    ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp, ExternalToolSurfaceSnapshot,
    ExternalToolSurfaceStagedOp,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const PENDING_CHANNEL_CAPACITY: usize = 32;

/// MCP server lifecycle state used by staged router apply.
///
/// This is a public API projection of the surface owner's internal state.
/// The canonical truth lives in the active owner path; this enum is derived
/// from it for backward-compatible API consumers.
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

/// Typed per-server boundary-apply rejection (K14).
///
/// Carried on [`McpApplyDelta`] so a partially-rejected boundary apply is a
/// typed per-server fact in the result, not a `tracing::warn!`-and-continue.
#[derive(Debug, Clone)]
pub struct McpBoundaryRejection {
    pub server: String,
    pub error: ExternalToolSurfaceError,
}

/// Result of applying staged MCP operations.
#[derive(Debug, Clone, Default)]
pub struct McpApplyDelta {
    pub added_servers: Vec<String>,
    pub removed_servers: Vec<String>,
    pub reloaded_servers: Vec<String>,
    pub lifecycle_actions: Vec<McpLifecycleAction>,
    pub degraded_removals: Vec<String>,
    /// Staged intents whose `ApplyBoundary` the surface owner rejected.
    pub rejected_boundaries: Vec<McpBoundaryRejection>,
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

/// Fail-closed surface handle used until generated machine authority is bound.
struct UnboundExternalToolSurfaceHandle;

impl UnboundExternalToolSurfaceHandle {
    fn new() -> Self {
        Self
    }

    fn reject(context: &'static str) -> DslTransitionError {
        DslTransitionError::guard_rejected(
            context,
            "external tool surface lifecycle requires generated machine authority",
        )
    }

    fn empty_snapshot() -> SurfaceDiagnosticSnapshot {
        SurfaceDiagnosticSnapshot {
            surface_phase: ExternalToolSurfaceGlobalPhase::Operating,
            known_surfaces: BTreeSet::new(),
            visible_surfaces: BTreeSet::new(),
            snapshot_epoch: 0,
            snapshot_aligned_epoch: 0,
            has_pending_or_staged: false,
            entries: Vec::new(),
        }
    }
}

impl ExternalToolSurfaceHandle for UnboundExternalToolSurfaceHandle {
    fn apply_surface_input(
        &self,
        _input: CoreSurfaceInput,
    ) -> Result<CoreSurfaceTransition, DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::apply_surface_input",
        ))
    }

    fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject("UnboundExternalToolSurfaceHandle::register"))
    }

    fn stage_add(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject("UnboundExternalToolSurfaceHandle::stage_add"))
    }

    fn stage_remove(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::stage_remove",
        ))
    }

    fn stage_reload(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::stage_reload",
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
            "UnboundExternalToolSurfaceHandle::apply_boundary",
        ))
    }

    fn mark_pending_succeeded(
        &self,
        _surface_id: String,
        _pending_task_sequence: u64,
        _staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::mark_pending_succeeded",
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
            "UnboundExternalToolSurfaceHandle::mark_pending_failed",
        ))
    }

    fn call_started(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::call_started",
        ))
    }

    fn call_finished(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::call_finished",
        ))
    }

    fn finalize_removal_clean(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::finalize_removal_clean",
        ))
    }

    fn finalize_removal_forced(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::finalize_removal_forced",
        ))
    }

    fn snapshot_aligned(&self, _epoch: u64) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::snapshot_aligned",
        ))
    }

    fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
        Err(Self::reject(
            "UnboundExternalToolSurfaceHandle::shutdown_surface",
        ))
    }

    fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
        None
    }

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
        Self::empty_snapshot()
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

enum SurfaceOwner {
    Runtime {
        handle_slot: Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>,
    },
}

impl SurfaceOwner {
    fn runtime(handle: Arc<dyn ExternalToolSurfaceHandle>) -> Self {
        Self::Runtime {
            handle_slot: Arc::new(StdRwLock::new(handle)),
        }
    }

    fn runtime_handle_slot(&self) -> Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>> {
        match self {
            Self::Runtime { handle_slot, .. } => Some(Arc::clone(handle_slot)),
        }
    }

    fn runtime_handle(&self) -> Option<Arc<dyn ExternalToolSurfaceHandle>> {
        match self {
            Self::Runtime { handle_slot, .. } => Some(
                handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        }
    }

    fn diagnostic_snapshot(&self) -> ExternalToolSurfaceSnapshot {
        match self {
            Self::Runtime { handle_slot, .. } => {
                let handle = handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                snapshot_from_handle(handle.diagnostic_snapshot())
            }
        }
    }

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
        match self {
            Self::Runtime { .. } => self
                .runtime_handle()
                .and_then(|handle| handle.surface_snapshot(surface_id)),
        }
    }

    fn set_removal_timeout(
        &mut self,
        removal_timeout: Duration,
    ) -> Result<(), ExternalToolSurfaceError> {
        match self {
            Self::Runtime { handle_slot } => {
                let timeout_ms = removal_timeout.as_millis().min(u128::from(u64::MAX)) as u64;
                let handle = handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                handle
                    .apply_surface_input(CoreSurfaceInput::SetRemovalTimeout { timeout_ms })
                    .map(|_| ())
                    .map_err(|error| runtime_surface_error("SetRemovalTimeout", error))
            }
        }
    }

    fn apply(
        &self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        match self {
            Self::Runtime { handle_slot } => {
                let handle = handle_slot
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Self::apply_runtime(handle.as_ref(), input)
            }
        }
    }

    fn apply_runtime(
        handle: &dyn ExternalToolSurfaceHandle,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        let (input_name, core_input, applied_at_turn) = match input {
            ExternalToolSurfaceInput::StageAdd { surface_id } => (
                "StageAdd",
                CoreSurfaceInput::StageAdd {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                },
                None,
            ),
            ExternalToolSurfaceInput::StageRemove { surface_id } => (
                "StageRemove",
                CoreSurfaceInput::StageRemove {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                },
                None,
            ),
            ExternalToolSurfaceInput::StageReload { surface_id } => (
                "StageReload",
                CoreSurfaceInput::StageReload {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                },
                None,
            ),
            ExternalToolSurfaceInput::ApplyBoundary {
                surface_id,
                staged_intent_sequence,
                applied_at_turn,
            } => (
                "ApplyBoundary",
                CoreSurfaceInput::ApplyBoundary {
                    surface_id: surface_id.0,
                    now_ms: McpRouter::now_ms(),
                    staged_intent_sequence,
                    applied_at_turn: applied_at_turn.0,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::PendingSucceeded {
                surface_id,
                operation: _operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => (
                "PendingSucceeded",
                CoreSurfaceInput::MarkPendingSucceeded {
                    surface_id: surface_id.0,
                    pending_task_sequence,
                    staged_intent_sequence,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::PendingFailed {
                surface_id,
                operation: _operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
                cause,
            } => (
                "PendingFailed",
                CoreSurfaceInput::MarkPendingFailed {
                    surface_id: surface_id.0,
                    pending_task_sequence,
                    staged_intent_sequence,
                    cause,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::CallStarted { surface_id } => (
                "CallStarted",
                CoreSurfaceInput::CallStarted {
                    surface_id: surface_id.0,
                },
                None,
            ),
            ExternalToolSurfaceInput::CallFinished { surface_id } => (
                "CallFinished",
                CoreSurfaceInput::CallFinished {
                    surface_id: surface_id.0,
                },
                None,
            ),
            ExternalToolSurfaceInput::FinalizeRemovalClean {
                surface_id,
                applied_at_turn,
            } => (
                "FinalizeRemovalClean",
                CoreSurfaceInput::FinalizeRemovalClean {
                    surface_id: surface_id.0,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id,
                applied_at_turn,
            } => (
                "FinalizeRemovalForced",
                CoreSurfaceInput::FinalizeRemovalForced {
                    surface_id: surface_id.0,
                },
                Some(applied_at_turn),
            ),
            ExternalToolSurfaceInput::SnapshotAligned { snapshot_epoch } => (
                "SnapshotAligned",
                CoreSurfaceInput::SnapshotAligned {
                    epoch: snapshot_epoch,
                },
                None,
            ),
            ExternalToolSurfaceInput::Shutdown => ("Shutdown", CoreSurfaceInput::Shutdown, None),
        };
        handle
            .apply_surface_input(core_input)
            .map(|transition| runtime_transition_from_core(input_name, transition, applied_at_turn))
            .map_err(|error| runtime_surface_error(input_name, error))
    }

    fn staged_intents_in_order(&self) -> Vec<(SurfaceId, StagedSurfaceOp, u64)> {
        match self {
            Self::Runtime { .. } => {
                let mut staged = self
                    .diagnostic_snapshot()
                    .entries
                    .into_iter()
                    .filter_map(|entry| {
                        let staged_op = staged_surface_op_from_snapshot(entry.staged_op);
                        if staged_op == StagedSurfaceOp::None {
                            return None;
                        }
                        Some((
                            SurfaceId::from(entry.surface_id),
                            staged_op,
                            entry.staged_intent_sequence,
                        ))
                    })
                    .collect::<Vec<_>>();
                staged.sort_by_key(|(_, _, sequence)| *sequence);
                staged
            }
        }
    }

    fn pending_count(&self) -> usize {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.pending_op != ExternalToolSurfacePendingOp::None)
            .count()
    }

    fn pending_surfaces(&self) -> Vec<SurfaceId> {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.pending_op != ExternalToolSurfacePendingOp::None)
            .map(|entry| SurfaceId::from(entry.surface_id))
            .collect()
    }

    fn visible_surfaces(&self) -> Vec<SurfaceId> {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.visible)
            .map(|entry| SurfaceId::from(entry.surface_id))
            .collect()
    }

    fn removing_surfaces(&self) -> Vec<SurfaceId> {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.base_state == ExternalToolSurfaceBaseState::Removing)
            .map(|entry| SurfaceId::from(entry.surface_id))
            .collect()
    }

    fn snapshot_epoch(&self) -> u64 {
        self.diagnostic_snapshot().snapshot_epoch
    }

    fn surface_base(&self, id: &SurfaceId) -> SurfaceBaseState {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| surface_base_from_snapshot(entry.base_state))
            .unwrap_or(SurfaceBaseState::Absent)
    }

    fn inflight_call_count(&self, id: &SurfaceId) -> u64 {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| entry.inflight_call_count)
            .unwrap_or(0)
    }

    fn removal_timing(&self, id: &SurfaceId) -> Option<RemovalTimingInfo> {
        match self {
            Self::Runtime { .. } => {
                let entry = self.surface_snapshot(&id.0)?;
                let draining_since_ms = entry.removal_draining_since_ms?;
                let timeout_at_ms = entry.removal_timeout_at_ms?;
                Some(RemovalTimingInfo {
                    draining_since: instant_from_epoch_ms(draining_since_ms),
                    timeout_at: instant_from_epoch_ms(timeout_at_ms),
                    applied_at_turn: TurnNumber(entry.removal_applied_at_turn.unwrap_or(0)),
                })
            }
        }
    }
}

fn runtime_transition_from_core(
    transition_name: &str,
    transition: CoreSurfaceTransition,
    applied_at_turn: Option<TurnNumber>,
) -> ExternalToolSurfaceTransition {
    ExternalToolSurfaceTransition {
        transition_name: transition_name.to_string(),
        phase: phase_from_snapshot(transition.phase),
        effects: transition
            .effects
            .into_iter()
            .map(|effect| runtime_effect_from_core(effect, applied_at_turn))
            .collect(),
    }
}

fn runtime_effect_from_core(
    effect: CoreSurfaceEffect,
    applied_at_turn: Option<TurnNumber>,
) -> ExternalToolSurfaceEffect {
    match effect {
        CoreSurfaceEffect::ScheduleSurfaceCompletion {
            surface_id,
            operation,
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn,
        } => ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
            surface_id: SurfaceId::from(surface_id),
            operation: surface_delta_operation_from_core(operation),
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn: TurnNumber(applied_at_turn),
        },
        CoreSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch } => {
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch }
        }
        CoreSurfaceEffect::EmitExternalToolDelta {
            surface_id,
            operation,
            phase,
            cause,
        } => ExternalToolSurfaceEffect::EmitExternalToolDelta {
            surface_id: SurfaceId::from(surface_id),
            operation: surface_delta_operation_from_core(operation),
            phase: surface_delta_phase_from_core(phase),
            cause,
            persisted: matches!(
                phase,
                ExternalToolSurfaceDeltaPhase::Applied
                    | ExternalToolSurfaceDeltaPhase::Failed
                    | ExternalToolSurfaceDeltaPhase::Forced
            ),
            applied_at_turn: applied_at_turn.unwrap_or(TurnNumber(0)),
        },
        CoreSurfaceEffect::CloseSurfaceConnection { surface_id } => {
            ExternalToolSurfaceEffect::CloseSurfaceConnection {
                surface_id: SurfaceId::from(surface_id),
            }
        }
        CoreSurfaceEffect::RejectSurfaceCall { surface_id, cause } => {
            ExternalToolSurfaceEffect::RejectSurfaceCall {
                surface_id: SurfaceId::from(surface_id),
                cause,
            }
        }
    }
}

fn runtime_surface_error(input_name: &str, error: DslTransitionError) -> ExternalToolSurfaceError {
    ExternalToolSurfaceError {
        input_name: input_name.to_string(),
        reason: error.to_string(),
    }
}

fn phase_from_snapshot(phase: ExternalToolSurfaceGlobalPhase) -> ExternalToolSurfacePhase {
    match phase {
        ExternalToolSurfaceGlobalPhase::Operating => ExternalToolSurfacePhase::Operating,
        ExternalToolSurfaceGlobalPhase::Shutdown => ExternalToolSurfacePhase::Shutdown,
    }
}

fn surface_delta_operation_from_core(
    operation: ExternalToolSurfaceDeltaOperation,
) -> SurfaceDeltaOperation {
    match operation {
        ExternalToolSurfaceDeltaOperation::None => SurfaceDeltaOperation::None,
        ExternalToolSurfaceDeltaOperation::Add => SurfaceDeltaOperation::Add,
        ExternalToolSurfaceDeltaOperation::Remove => SurfaceDeltaOperation::Remove,
        ExternalToolSurfaceDeltaOperation::Reload => SurfaceDeltaOperation::Reload,
    }
}

fn surface_delta_phase_from_core(phase: ExternalToolSurfaceDeltaPhase) -> SurfaceDeltaPhase {
    match phase {
        ExternalToolSurfaceDeltaPhase::None => SurfaceDeltaPhase::None,
        ExternalToolSurfaceDeltaPhase::Pending => SurfaceDeltaPhase::Pending,
        ExternalToolSurfaceDeltaPhase::Applied => SurfaceDeltaPhase::Applied,
        ExternalToolSurfaceDeltaPhase::Draining => SurfaceDeltaPhase::Draining,
        ExternalToolSurfaceDeltaPhase::Failed => SurfaceDeltaPhase::Failed,
        ExternalToolSurfaceDeltaPhase::Forced => SurfaceDeltaPhase::Forced,
    }
}

fn surface_base_from_snapshot(state: ExternalToolSurfaceBaseState) -> SurfaceBaseState {
    match state {
        ExternalToolSurfaceBaseState::Absent => SurfaceBaseState::Absent,
        ExternalToolSurfaceBaseState::Active => SurfaceBaseState::Active,
        ExternalToolSurfaceBaseState::Removing => SurfaceBaseState::Removing,
        ExternalToolSurfaceBaseState::Removed => SurfaceBaseState::Removed,
    }
}

fn staged_surface_op_from_snapshot(op: ExternalToolSurfaceStagedOp) -> StagedSurfaceOp {
    match op {
        ExternalToolSurfaceStagedOp::None => StagedSurfaceOp::None,
        ExternalToolSurfaceStagedOp::Add => StagedSurfaceOp::Add,
        ExternalToolSurfaceStagedOp::Remove => StagedSurfaceOp::Remove,
        ExternalToolSurfaceStagedOp::Reload => StagedSurfaceOp::Reload,
    }
}

fn instant_from_epoch_ms(epoch_ms: u64) -> Instant {
    let now_ms = McpRouter::now_ms();
    let now = Instant::now();
    if epoch_ms >= now_ms {
        now.checked_add(Duration::from_millis(epoch_ms - now_ms))
            .unwrap_or(now)
    } else {
        now.checked_sub(Duration::from_millis(now_ms - epoch_ms))
            .unwrap_or(now)
    }
}

fn snapshot_from_handle(snapshot: SurfaceDiagnosticSnapshot) -> ExternalToolSurfaceSnapshot {
    let SurfaceDiagnosticSnapshot {
        surface_phase,
        visible_surfaces,
        snapshot_epoch,
        snapshot_aligned_epoch,
        entries,
        ..
    } = snapshot;
    let entries = entries
        .into_iter()
        .map(|entry| entry_snapshot_from_handle(entry, &visible_surfaces))
        .collect();

    ExternalToolSurfaceSnapshot {
        phase: surface_phase,
        snapshot_epoch,
        snapshot_aligned_epoch,
        entries,
    }
}

fn entry_snapshot_from_handle(
    entry: SurfaceSnapshot,
    visible_surfaces: &BTreeSet<String>,
) -> ExternalToolSurfaceEntrySnapshot {
    ExternalToolSurfaceEntrySnapshot {
        visible: visible_surfaces.contains(&entry.surface_id),
        surface_id: entry.surface_id,
        // Typed cross-crate handle contract: `None` means the DSL never
        // recorded a value for this surface, so the projection defaults
        // to the `Absent` / `None` variant per the contract invariants.
        base_state: entry
            .base_state
            .unwrap_or(ExternalToolSurfaceBaseState::Absent),
        has_removal_timing: entry.removal_draining_since_ms.is_some()
            || entry.removal_timeout_at_ms.is_some()
            || entry.removal_applied_at_turn.is_some(),
        pending_op: entry.pending_op,
        staged_op: entry.staged_op,
        staged_intent_sequence: entry.staged_intent_sequence.unwrap_or(0),
        pending_task_sequence: entry.pending_task_sequence.unwrap_or(0),
        pending_lineage_sequence: entry.pending_lineage_sequence.unwrap_or(0),
        inflight_call_count: entry.inflight_calls,
        last_delta_operation: entry
            .last_delta_operation
            .unwrap_or(ExternalToolSurfaceDeltaOperation::None),
        last_delta_phase: entry
            .last_delta_phase
            .unwrap_or(ExternalToolSurfaceDeltaPhase::None),
    }
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
    let core_effects = core_surface_effects(effects);
    let mut latest = None;
    for obligation in protocol_surface_snapshot_alignment::extract_obligations(&core_effects) {
        merge_snapshot_alignment(&mut latest, obligation);
    }
    latest
}

fn core_surface_operation(op: SurfaceDeltaOperation) -> ExternalToolSurfaceDeltaOperation {
    match op {
        SurfaceDeltaOperation::None => ExternalToolSurfaceDeltaOperation::None,
        SurfaceDeltaOperation::Add => ExternalToolSurfaceDeltaOperation::Add,
        SurfaceDeltaOperation::Remove => ExternalToolSurfaceDeltaOperation::Remove,
        SurfaceDeltaOperation::Reload => ExternalToolSurfaceDeltaOperation::Reload,
    }
}

fn local_surface_operation(op: ExternalToolSurfaceDeltaOperation) -> SurfaceDeltaOperation {
    match op {
        ExternalToolSurfaceDeltaOperation::None => SurfaceDeltaOperation::None,
        ExternalToolSurfaceDeltaOperation::Add => SurfaceDeltaOperation::Add,
        ExternalToolSurfaceDeltaOperation::Remove => SurfaceDeltaOperation::Remove,
        ExternalToolSurfaceDeltaOperation::Reload => SurfaceDeltaOperation::Reload,
    }
}

/// Map a completion-obligation operation to the lifecycle-action vocabulary.
///
/// Completion obligations only exist for Add/Reload pending spawns; `Remove`
/// never spawns a pending task and `None` is unreachable on an obligation.
/// Both map to `Add` so a fault report on a malformed obligation still names
/// a concrete operation rather than being dropped.
fn obligation_lifecycle_operation(
    op: ExternalToolSurfaceDeltaOperation,
) -> ToolConfigChangeOperation {
    match op {
        ExternalToolSurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
        ExternalToolSurfaceDeltaOperation::Remove => ToolConfigChangeOperation::Remove,
        ExternalToolSurfaceDeltaOperation::Add | ExternalToolSurfaceDeltaOperation::None => {
            ToolConfigChangeOperation::Add
        }
    }
}

fn core_surface_effects(effects: &[ExternalToolSurfaceEffect]) -> Vec<CoreSurfaceEffect> {
    effects
        .iter()
        .map(|effect| match effect {
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => CoreSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id: surface_id.0.clone(),
                operation: core_surface_operation(*operation),
                pending_task_sequence: *pending_task_sequence,
                staged_intent_sequence: *staged_intent_sequence,
                applied_at_turn: applied_at_turn.0,
            },
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch } => {
                CoreSurfaceEffect::RefreshVisibleSurfaceSet {
                    snapshot_epoch: *snapshot_epoch,
                }
            }
            ExternalToolSurfaceEffect::EmitExternalToolDelta {
                surface_id,
                operation,
                phase,
                cause,
                ..
            } => CoreSurfaceEffect::EmitExternalToolDelta {
                surface_id: surface_id.0.clone(),
                operation: core_surface_operation(*operation),
                phase: match phase {
                    SurfaceDeltaPhase::None => ExternalToolSurfaceDeltaPhase::None,
                    SurfaceDeltaPhase::Pending => ExternalToolSurfaceDeltaPhase::Pending,
                    SurfaceDeltaPhase::Applied => ExternalToolSurfaceDeltaPhase::Applied,
                    SurfaceDeltaPhase::Draining => ExternalToolSurfaceDeltaPhase::Draining,
                    SurfaceDeltaPhase::Failed => ExternalToolSurfaceDeltaPhase::Failed,
                    SurfaceDeltaPhase::Forced => ExternalToolSurfaceDeltaPhase::Forced,
                },
                cause: *cause,
            },
            ExternalToolSurfaceEffect::CloseSurfaceConnection { surface_id } => {
                CoreSurfaceEffect::CloseSurfaceConnection {
                    surface_id: surface_id.0.clone(),
                }
            }
            ExternalToolSurfaceEffect::RejectSurfaceCall { surface_id, cause } => {
                CoreSurfaceEffect::RejectSurfaceCall {
                    surface_id: surface_id.0.clone(),
                    cause: *cause,
                }
            }
        })
        .collect()
}

fn lifecycle_operation_from_effects(
    effects: &[ExternalToolSurfaceEffect],
    expected_phase: SurfaceDeltaPhase,
) -> Option<ToolConfigChangeOperation> {
    effects.iter().find_map(|effect| match effect {
        ExternalToolSurfaceEffect::EmitExternalToolDelta {
            operation, phase, ..
        } if *phase == expected_phase => match operation {
            SurfaceDeltaOperation::Add => Some(ToolConfigChangeOperation::Add),
            SurfaceDeltaOperation::Remove => Some(ToolConfigChangeOperation::Remove),
            SurfaceDeltaOperation::Reload => Some(ToolConfigChangeOperation::Reload),
            SurfaceDeltaOperation::None => None,
        },
        _ => None,
    })
}

#[derive(Debug, Clone)]
struct RouterProjectionSnapshot {
    #[allow(dead_code)]
    epoch: u64,
    tool_to_server: HashMap<String, String>,
    catalog_entries: Arc<[ToolCatalogEntry]>,
    visible_tools: Arc<[Arc<ToolDef>]>,
}

impl Default for RouterProjectionSnapshot {
    fn default() -> Self {
        Self {
            epoch: 0,
            tool_to_server: HashMap::new(),
            catalog_entries: Arc::from([]),
            visible_tools: Arc::from([]),
        }
    }
}

/// Shell-level server entry. Holds connection, config, and tools.
/// Lifecycle state is owned by the surface owner, not by this struct.
struct ServerEntry {
    config: McpServerConfig,
    connection: Option<McpConnection>,
    tools: Vec<Arc<ToolDef>>,
    /// Shell-level atomic for inflight call RAII guards. The surface owner owns
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
/// All lifecycle state transitions flow through the active surface owner.
/// Runtime/session routers receive a runtime-owned handle from factory wiring;
/// standalone routers use a local compatibility handle. The router manages
/// connections, async tasks, tool caching, and effect execution.
///
/// Add and reload operations are non-blocking: `apply_staged()` spawns
/// background connection tasks and returns immediately. Completions are
/// delivered via [`take_external_updates`](Self::take_external_updates).
pub struct McpRouter {
    surface_owner: SurfaceOwner,

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
    /// Optional session-scoped MCP server lifecycle handle
    /// (Phase 5G / T5g). When bound, every handshake event mirrors into
    /// the session's MeerkatMachine DSL `mcp_server_states`. Standalone
    /// callers (tests, fixtures) leave this `None` and the DSL mirror is
    /// skipped — shell-level behavior stays identical.
    ///
    /// Stored behind `Arc<StdRwLock<...>>` so the adapter (which wraps the
    /// router in a `tokio` async lock) can bind the handle late — after
    /// construction, via a sync trait method — without needing to block on
    /// the async router lock.
    mcp_lifecycle_handle: Arc<StdRwLock<Option<Arc<dyn McpServerLifecycleHandle>>>>,
    mcp_auth_mode: McpAuthMode,
    mcp_auth_resolver: Option<Arc<dyn McpAuthResolver>>,
}

impl McpRouter {
    fn with_surface_owner(surface_owner: SurfaceOwner) -> Self {
        let (tx, rx) = mpsc::channel(PENDING_CHANNEL_CAPACITY);
        Self {
            surface_owner,
            servers: HashMap::new(),
            projection: Arc::new(RouterProjectionSnapshot::default()),
            staged_payloads: HashMap::new(),
            pending_tx: tx,
            pending_rx: Mutex::new(rx),
            pending_obligations: HashMap::new(),
            pending_snapshot_alignment: None,
            completed_updates: VecDeque::new(),
            mcp_lifecycle_handle: Arc::new(StdRwLock::new(None)),
            mcp_auth_mode: McpAuthMode::Stored,
            mcp_auth_resolver: None,
        }
    }

    /// Return the shared handle slot for late-binding a session's MCP server
    /// lifecycle DSL handle (Phase 5G / T5g). The adapter (which wraps the
    /// router in a tokio async lock) calls this once at router construction
    /// to obtain a clone it can write into from the sync
    /// `AgentToolDispatcher::bind_mcp_server_lifecycle_handle` method.
    pub fn mcp_lifecycle_handle_slot(
        &self,
    ) -> Arc<StdRwLock<Option<Arc<dyn McpServerLifecycleHandle>>>> {
        Arc::clone(&self.mcp_lifecycle_handle)
    }

    /// Return the shared runtime surface-handle slot. Runtime-backed session
    /// builds replace the router's default ephemeral DSL owner with the
    /// session-owned MeerkatMachine handle through this slot.
    pub fn external_surface_handle_slot(
        &self,
    ) -> Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>> {
        self.surface_owner.runtime_handle_slot()
    }

    /// Route a lifecycle transition into the bound session DSL mirror.
    ///
    /// K14: rejected applies are typed faults, not debug-swallowed log lines.
    /// Synchronous ingress paths (`stage_add`, `stage_reload`) propagate the
    /// `Err` directly; async completion paths convert it into a `Failed`
    /// lifecycle action on the canonical action channel. With no handle bound
    /// (standalone routers, tests) the mirror is absent by construction and
    /// the apply is `Ok`.
    fn with_lifecycle_handle<F>(&self, server_name: &str, f: F) -> Result<(), McpError>
    where
        F: FnOnce(&dyn McpServerLifecycleHandle) -> Result<(), DslTransitionError>,
    {
        let guard = match self.mcp_lifecycle_handle.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(
                    "mcp_lifecycle_handle RwLock poisoned; recovering — DSL mirror may drift"
                );
                poisoned.into_inner()
            }
        };
        match guard.as_deref() {
            None => Ok(()),
            Some(handle) => f(handle).map_err(|source| McpError::LifecycleMirrorRejected {
                server: server_name.to_string(),
                source,
            }),
        }
    }

    fn notify_lifecycle_connect_pending(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| {
            handle.apply_connect_pending(server_name)
        })
    }

    fn notify_lifecycle_connected(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| handle.apply_connected(server_name))
    }

    fn notify_lifecycle_failed(&self, server_name: &str, failure: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| {
            handle.apply_failed(server_name, failure)
        })
    }

    fn notify_lifecycle_disconnected(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| handle.apply_disconnected(server_name))
    }

    fn notify_lifecycle_reload(&self, server_name: &str) -> Result<(), McpError> {
        self.with_lifecycle_handle(server_name, |handle| handle.apply_reload(server_name))
    }

    /// Create a new empty router without a generated surface authority.
    ///
    /// Lifecycle mutations fail closed until runtime/session construction binds
    /// a generated [`ExternalToolSurfaceHandle`] through
    /// [`Self::new_with_surface_handle`] or late adapter binding.
    pub fn new() -> Self {
        let surface_handle: Arc<dyn ExternalToolSurfaceHandle> =
            Arc::new(UnboundExternalToolSurfaceHandle::new());
        Self::with_surface_owner(SurfaceOwner::runtime(surface_handle))
    }

    /// Create a new empty router with a runtime-backed surface handle.
    pub fn new_with_surface_handle(surface_handle: Arc<dyn ExternalToolSurfaceHandle>) -> Self {
        Self::with_surface_owner(SurfaceOwner::runtime(surface_handle))
    }

    pub fn with_mcp_auth(
        mut self,
        mode: McpAuthMode,
        resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Self {
        self.mcp_auth_mode = mode;
        self.mcp_auth_resolver = resolver;
        self
    }

    /// Create a new router with a custom remove-drain timeout.
    pub fn new_with_removal_timeout(removal_timeout: Duration) -> Self {
        let mut router = Self::new();
        if let Err(error) = router.set_removal_timeout(removal_timeout) {
            tracing::warn!(
                timeout_ms = removal_timeout.as_millis(),
                error = %error,
                "Surface owner rejected set_removal_timeout during router construction"
            );
        }
        router
    }

    /// Create a new router with a custom remove-drain timeout and runtime-backed surface handle.
    pub fn new_with_surface_handle_and_removal_timeout(
        surface_handle: Arc<dyn ExternalToolSurfaceHandle>,
        removal_timeout: Duration,
    ) -> Self {
        let mut router = Self::new_with_surface_handle(surface_handle);
        if let Err(error) = router.set_removal_timeout(removal_timeout) {
            tracing::warn!(
                timeout_ms = removal_timeout.as_millis(),
                error = %error,
                "Surface owner rejected set_removal_timeout during router construction"
            );
        }
        router
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }

    fn turn_number_from_staged_sequence(&self, staged_sequence: u64) -> TurnNumber {
        if staged_sequence > 0 {
            TurnNumber(staged_sequence)
        } else {
            // Defensive fallback: staged intents should always have non-zero sequence.
            TurnNumber(self.surface_owner.snapshot_epoch())
        }
    }

    fn turn_number_from_snapshot_epoch(&self) -> TurnNumber {
        TurnNumber(self.surface_owner.snapshot_epoch())
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
    pub fn set_removal_timeout(
        &mut self,
        removal_timeout: Duration,
    ) -> Result<(), ExternalToolSurfaceError> {
        self.surface_owner.set_removal_timeout(removal_timeout)
    }

    /// Snapshot the live external tool-surface state.
    pub fn external_tool_surface_snapshot(&self) -> meerkat_core::ExternalToolSurfaceSnapshot {
        self.surface_owner.diagnostic_snapshot()
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
    pub fn stage_add(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd { surface_id: sid })
        {
            Ok(_) => {
                // K14: the lifecycle DSL mirror must accept the transition
                // before any shell mirror mutation. On rejection the staged
                // payload is NOT installed and the typed fault propagates;
                // the owner-side staged intent then fails explicitly at
                // `apply_staged` with a missing-payload protocol error.
                self.notify_lifecycle_connect_pending(&server_name)?;
                self.staged_payloads.insert(server_name, config);
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageAdd"
                );
                Err(error.into())
            }
        }
    }

    /// Stage a server remove intent for the next boundary apply.
    ///
    /// This only records intent. Removal lifecycle starts on `apply_staged`.
    pub fn stage_remove(&mut self, server_name: impl Into<String>) -> Result<(), McpError> {
        let server_name = server_name.into();
        let sid = SurfaceId::from(server_name.as_str());
        if let Err(error) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageRemove { surface_id: sid })
        {
            tracing::warn!(
                server = %server_name,
                error = %error,
                "Surface owner rejected StageRemove"
            );
            return Err(error.into());
        }
        self.staged_payloads.remove(&server_name);
        Ok(())
    }

    /// Stage a server reload by server name (reuse existing config) or full config.
    pub fn stage_reload<T: Into<McpReloadTarget>>(&mut self, target: T) -> Result<(), McpError> {
        let (server_name, requested_payload) = match target.into() {
            McpReloadTarget::ServerName(server_name) => (server_name, None),
            McpReloadTarget::Config(config) => (config.name.clone(), Some(config)),
        };
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageReload { surface_id: sid })
        {
            Ok(_) => {
                // K14: rejected mirror apply propagates typed; the staged
                // payload bookkeeping below only runs on accepted apply.
                self.notify_lifecycle_reload(&server_name)?;
                // Lifecycle legality is owner-owned. Shell payload lookup is
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
                        "Surface owner accepted StageReload but no execution payload is available; apply_staged will surface this as a protocol error"
                    );
                    self.staged_payloads.remove(&server_name);
                }
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageReload"
                );
                Err(error.into())
            }
        }
    }

    /// Apply staged operations at a boundary (non-blocking for add/reload).
    ///
    /// All state transitions flow through the surface owner. The shell spawns
    /// background tasks and executes effects.
    pub async fn apply_staged(&mut self) -> Result<McpApplyResult, McpError> {
        let mut delta = McpApplyDelta::default();
        // 1. Drain pending results from background tasks.
        self.drain_pending();
        let staged_intents = self.surface_owner.staged_intents_in_order();
        let mut snapshot_alignment = None;

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
                .surface_owner
                .apply(ExternalToolSurfaceInput::ApplyBoundary {
                    surface_id,
                    staged_intent_sequence: staged_sequence,
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
                        "Surface owner rejected ApplyBoundary for staged intent"
                    );
                    // K14: the rejection is a typed per-server fact on the
                    // apply result, not just a log line.
                    delta.rejected_boundaries.push(McpBoundaryRejection {
                        server: server_name,
                        error,
                    });
                }
            }
        }

        self.process_removals(&mut delta, &mut snapshot_alignment)
            .await?;
        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }

        let pending_count = self.surface_owner.pending_count();
        Ok(McpApplyResult {
            delta,
            pending_count,
        })
    }

    /// Execute effects returned by the surface owner.
    fn execute_effects(
        &mut self,
        effects: &[ExternalToolSurfaceEffect],
        delta: &mut McpApplyDelta,
        config: Option<&McpServerConfig>,
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        // Extract obligation tokens from ScheduleSurfaceCompletion effects
        // before iterating — these are consumed by spawn_pending.
        let core_effects = core_surface_effects(effects);
        let mut obligations = protocol_surface_completion::extract_obligations(&core_effects)
            .into_iter()
            .collect::<VecDeque<_>>();
        let mut snapshot_obligations =
            protocol_surface_snapshot_alignment::extract_obligations(&core_effects)
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
                    // K14: the close effect comes from an already-accepted
                    // surface transition, so it must execute; a rejected
                    // lifecycle mirror apply propagates as a typed Failed
                    // action on the canonical lifecycle channel instead of
                    // being debug-swallowed.
                    if let Err(error) = self.notify_lifecycle_disconnected(&surface_id.0) {
                        self.completed_updates.push_back(CompletedLifecycleUpdate {
                            action: McpLifecycleAction::new(
                                surface_id.0.clone(),
                                ToolConfigChangeOperation::Remove,
                                McpLifecyclePhase::Failed,
                            )
                            .with_detail(Some(error.to_string())),
                        });
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
            .insert(server_name, obligation.clone());

        let tx = self.pending_tx.clone();
        let auth_mode = self.mcp_auth_mode;
        let auth_resolver = self.mcp_auth_resolver.clone();
        tokio::spawn(async move {
            let result = McpConnection::connect_and_enumerate_with_mcp_auth(
                &config,
                auth_mode,
                auth_resolver,
            )
            .await;
            if let Err(error) = tx.send(PendingResult { obligation, result }).await {
                let server_name = error.0.obligation.surface_id.clone();
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
        let server_name = obligation.surface_id.clone();

        match result {
            Ok((conn, tools)) => {
                let tool_count = tools.len();

                match self
                    .surface_owner
                    .apply(ExternalToolSurfaceInput::PendingSucceeded {
                        surface_id: SurfaceId(obligation.surface_id.clone()),
                        operation: local_surface_operation(obligation.operation),
                        pending_task_sequence: obligation.pending_task_sequence,
                        staged_intent_sequence: obligation.staged_intent_sequence,
                        applied_at_turn: TurnNumber(obligation.applied_at_turn),
                    }) {
                    Ok(transition) => {
                        self.pending_obligations.remove(&server_name);
                        let Some(operation) = lifecycle_operation_from_effects(
                            &transition.effects,
                            SurfaceDeltaPhase::Applied,
                        ) else {
                            tracing::error!(
                                server = %server_name,
                                "Surface owner accepted PendingSucceeded without an applied lifecycle delta"
                            );
                            return None;
                        };
                        let snapshot_alignment = latest_snapshot_alignment(&transition.effects);

                        // K14: the lifecycle DSL mirror must accept the
                        // connected transition before the shell installs the
                        // server entry. On rejection the fresh connection is
                        // closed, the shell mirror stays unchanged, and the
                        // typed fault propagates as a Failed action on the
                        // canonical lifecycle channel (the same channel
                        // background connection failures use).
                        if let Err(error) = self.notify_lifecycle_connected(&server_name) {
                            let name = server_name.clone();
                            tokio::spawn(async move {
                                if let Err(close_error) = conn.close().await {
                                    tracing::debug!(
                                        "Error closing MCP connection '{}' after rejected lifecycle mirror apply: {}",
                                        name,
                                        close_error
                                    );
                                }
                            });
                            self.completed_updates.push_back(CompletedLifecycleUpdate {
                                action: McpLifecycleAction::new(
                                    server_name,
                                    operation,
                                    McpLifecyclePhase::Failed,
                                )
                                .with_detail(Some(error.to_string())),
                            });
                            return snapshot_alignment;
                        }

                        // For reload: close old connection.
                        if obligation.operation == ExternalToolSurfaceDeltaOperation::Reload
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
                            && obligation.operation == ExternalToolSurfaceDeltaOperation::Add
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
                        snapshot_alignment
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "Surface owner rejected PendingSucceeded"
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
                        None
                    }
                }
            }
            Err(err) => {
                // K14: a rejected mirror apply is a typed fault folded into
                // the Failed action's detail below (the canonical lifecycle
                // channel), never debug-swallowed.
                let mirror_fault = self
                    .notify_lifecycle_failed(&server_name, &err.to_string())
                    .err();

                let (snapshot_alignment, operation) = match self.surface_owner.apply(
                    ExternalToolSurfaceInput::PendingFailed {
                        surface_id: SurfaceId(obligation.surface_id.clone()),
                        operation: local_surface_operation(obligation.operation),
                        pending_task_sequence: obligation.pending_task_sequence,
                        staged_intent_sequence: obligation.staged_intent_sequence,
                        applied_at_turn: TurnNumber(obligation.applied_at_turn),
                        cause: ExternalToolSurfaceFailureCause::PendingFailed,
                    },
                ) {
                    Ok(transition) => {
                        self.pending_obligations.remove(&server_name);
                        let Some(operation) = lifecycle_operation_from_effects(
                            &transition.effects,
                            SurfaceDeltaPhase::Failed,
                        ) else {
                            tracing::error!(
                                server = %server_name,
                                "Surface owner accepted PendingFailed without a failed lifecycle delta"
                            );
                            return None;
                        };
                        (latest_snapshot_alignment(&transition.effects), operation)
                    }
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            error = %e,
                            "Surface owner rejected PendingFailed"
                        );
                        // Even when the surface owner rejects the failure
                        // input, a rejected lifecycle mirror apply must still
                        // surface typed on the canonical action channel.
                        if let Some(mirror_fault) = mirror_fault {
                            self.completed_updates.push_back(CompletedLifecycleUpdate {
                                action: McpLifecycleAction::new(
                                    server_name,
                                    obligation_lifecycle_operation(obligation.operation),
                                    McpLifecyclePhase::Failed,
                                )
                                .with_detail(Some(mirror_fault.to_string())),
                            });
                        }
                        return None;
                    }
                };

                tracing::warn!(
                    server = %server_name,
                    error = %err,
                    op = ?obligation.operation,
                    "MCP server background connection failed"
                );

                let detail = match mirror_fault {
                    None => err.to_string(),
                    Some(mirror_fault) => {
                        format!("{err}; {mirror_fault}")
                    }
                };
                self.completed_updates.push_back(CompletedLifecycleUpdate {
                    action: McpLifecycleAction::new(
                        server_name,
                        operation,
                        McpLifecyclePhase::Failed,
                    )
                    .with_detail(Some(detail)),
                });
                snapshot_alignment
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
        let pending = self.pending_sources_snapshot();

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
        self.surface_owner.pending_count() > 0 || !self.completed_updates.is_empty()
    }

    /// Snapshot the names of tool sources still connecting/loading.
    pub fn pending_sources_snapshot(&self) -> Vec<String> {
        self.surface_owner
            .pending_surfaces()
            .into_iter()
            .map(|surface_id| surface_id.0)
            .collect()
    }

    /// Backward-compatible immediate install path. Bypasses staged/boundary
    /// flow and directly drives the surface owner through Stage -> Apply -> Success.
    async fn install_active_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());

        if let Err(e) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
        {
            tracing::warn!(
                server = %server_name,
                error = %e,
                "Surface owner rejected StageAdd in install path"
            );
            return Err(McpError::ProtocolError {
                message: format!("surface owner rejected StageAdd: {e}"),
            });
        }
        let staged_sequence = self
            .surface_owner
            .staged_intents_in_order()
            .into_iter()
            .find(|(surface_id, _, _)| *surface_id == sid)
            .map(|(_, _, sequence)| sequence)
            .unwrap_or(0);
        let applied_at_turn = self.turn_number_from_staged_sequence(staged_sequence);
        let boundary_effects =
            match self
                .surface_owner
                .apply(ExternalToolSurfaceInput::ApplyBoundary {
                    surface_id: sid.clone(),
                    staged_intent_sequence: staged_sequence,
                    applied_at_turn,
                }) {
                Ok(t) => t.effects,
                Err(e) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "Surface owner rejected ApplyBoundary in install path"
                    );
                    return Err(McpError::ProtocolError {
                        message: format!("surface owner rejected ApplyBoundary: {e}"),
                    });
                }
            };
        // Extract and immediately consume the obligation for the synchronous install path.
        let core_boundary_effects = core_surface_effects(&boundary_effects);
        let mut obligations =
            protocol_surface_completion::extract_obligations(&core_boundary_effects);
        let obligation = match obligations.pop() {
            Some(obligation) => obligation,
            None => {
                return Err(McpError::ProtocolError {
                    message: "surface owner ApplyBoundary emitted no surface completion obligation"
                        .into(),
                });
            }
        };

        let result = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            self.mcp_auth_mode,
            self.mcp_auth_resolver.clone(),
        )
        .await;
        let connect_error = result.as_ref().err().map(ToString::to_string);
        let snapshot_alignment = self.process_pending_result(PendingResult { obligation, result });

        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }

        if let Some(message) = connect_error {
            return Err(McpError::ProtocolError { message });
        }

        if self.servers.contains_key(&server_name) {
            Ok(())
        } else {
            Err(McpError::ProtocolError {
                message: "surface owner did not activate MCP server after successful connect"
                    .into(),
            })
        }
    }

    /// Process removal finalization based on owner-owned timeout tracking.
    ///
    /// A surface-owner rejection of a finalize-removal input is authoritative
    /// divergence: the router has computed a finalized set the machine refuses
    /// to commit, so router/surface state would silently diverge if we
    /// continued. Fail closed by surfacing the rejection as a typed
    /// [`McpError`] instead of warn-and-continue.
    async fn process_removals(
        &mut self,
        delta: &mut McpApplyDelta,
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) -> Result<(), McpError> {
        let now = Instant::now();
        let mut finalized: Vec<(String, bool)> = Vec::new();

        let removing = self.surface_owner.removing_surfaces();
        for sid in &removing {
            let inflight = self.surface_owner.inflight_call_count(sid);
            let timing = self.surface_owner.removal_timing(sid);

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
            let applied_at_turn = match self.surface_owner.removal_timing(&sid) {
                Some(timing) => timing.applied_at_turn,
                None => {
                    tracing::warn!(
                        server = %server_name,
                        "removal finalization missing owner timing; falling back to snapshot epoch"
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
            let transition =
                self.surface_owner
                    .apply(input)
                    .map_err(|e| McpError::ProtocolError {
                        message: format!(
                            "surface owner rejected finalize removal for '{server_name}': {e}"
                        ),
                    })?;
            self.execute_effects(&transition.effects, delta, None, snapshot_alignment);
            if degraded {
                delta.degraded_removals.push(server_name);
            }
        }
        Ok(())
    }

    fn align_snapshot_if_requested(
        &mut self,
        snapshot_alignment: Option<SurfaceSnapshotAlignmentObligation>,
    ) {
        let Some(obligation) = snapshot_alignment else {
            return;
        };
        if let Err(error) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::SnapshotAligned {
                snapshot_epoch: obligation.snapshot_epoch,
            })
        {
            tracing::warn!(
                snapshot_epoch = obligation.snapshot_epoch,
                error = %error,
                "Surface owner rejected SnapshotAligned"
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
        let epoch = self.surface_owner.snapshot_epoch();
        let server_names: BTreeSet<String> = self
            .surface_owner
            .visible_surfaces()
            .into_iter()
            .map(|sid| sid.0)
            .collect();

        let mut tool_to_server = HashMap::new();
        let mut canonical_tools: BTreeMap<String, Arc<ToolDef>> = BTreeMap::new();
        let mut collided: BTreeSet<String> = BTreeSet::new();
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
                // Fail closed by exclusion: a tool name owned by two different
                // servers is ambiguous, so it must be dispatchable from NEITHER.
                // A same-name-same-server re-entry is not a collision.
                if let Some(existing_owner) = tool_to_server.get(tool.name.as_str())
                    && existing_owner != &server_name
                {
                    tracing::warn!(
                        tool = %tool.name,
                        existing_owner = %existing_owner,
                        colliding_owner = %server_name,
                        "MCP projection tool name owned by two servers; excluding it from the published catalog (ambiguous => denied)"
                    );
                    collided.insert(tool.name.to_string());
                    complete = false;
                    continue;
                }
                tool_to_server.insert(tool.name.to_string(), server_name.clone());
                canonical_tools.insert(tool.name.to_string(), Arc::clone(tool));
            }
        }

        // Remove every collided name so a tool exposed by two servers is
        // routable from neither (the first writer is also dropped here).
        for name in &collided {
            tool_to_server.remove(name);
            canonical_tools.remove(name);
        }

        let catalog_entries: Arc<[ToolCatalogEntry]> = canonical_tools
            .values()
            .map(|tool| {
                if let Some(provenance) = tool.provenance.clone() {
                    ToolCatalogEntry::session_deferred(Arc::clone(tool), true, provenance)
                } else {
                    ToolCatalogEntry::session_inline(Arc::clone(tool), true)
                }
            })
            .collect::<Vec<_>>()
            .into();
        let visible_tools: Arc<[Arc<ToolDef>]> = catalog_entries
            .iter()
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into();

        self.projection = Arc::new(RouterProjectionSnapshot {
            epoch,
            tool_to_server,
            catalog_entries,
            visible_tools,
        });
        complete
    }

    fn projection_tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.projection.visible_tools)
    }

    fn projection_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        Arc::clone(&self.projection.catalog_entries)
    }

    /// Get current lifecycle state for a server.
    ///
    /// Derives the lifecycle state from the surface owner's canonical state.
    pub fn server_lifecycle_state(&self, server_name: &str) -> Option<McpServerLifecycleState> {
        let sid = SurfaceId::from(server_name);
        let base = self.surface_owner.surface_base(&sid);
        match base {
            SurfaceBaseState::Active => Some(McpServerLifecycleState::Active),
            SurfaceBaseState::Removing => {
                let timing = self.surface_owner.removal_timing(&sid);
                match timing {
                    Some(t) => Some(McpServerLifecycleState::Removing {
                        draining_since: t.draining_since,
                        timeout_at: t.timeout_at,
                    }),
                    None => {
                        tracing::warn!(
                            server = %server_name,
                            "surface owner reported Removing without generated removal timing"
                        );
                        None
                    }
                }
            }
            SurfaceBaseState::Removed => Some(McpServerLifecycleState::Removed),
            SurfaceBaseState::Absent => None,
        }
    }

    /// List names of all active (non-removed) servers.
    pub fn active_server_names(&self) -> Vec<String> {
        self.surface_owner
            .visible_surfaces()
            .into_iter()
            .map(|sid| sid.0)
            .collect()
    }

    /// Returns true if any server is currently in Removing state.
    pub fn has_removing_servers(&self) -> bool {
        !self.surface_owner.removing_surfaces().is_empty()
    }

    /// List all visible tools from active servers.
    pub fn list_tools(&self) -> &[Arc<ToolDef>] {
        self.projection.visible_tools.as_ref()
    }

    /// Progress only server removals (drain/timeout finalization) without applying staged ops.
    ///
    /// Fails closed if the surface owner rejects a finalize-removal: a rejected
    /// finalize is authoritative divergence (router computed a finalized set the
    /// machine refuses to commit), not a benign no-op.
    pub async fn progress_removals(&mut self) -> Result<McpApplyDelta, McpError> {
        let mut delta = McpApplyDelta::default();
        let mut snapshot_alignment = None;
        self.process_removals(&mut delta, &mut snapshot_alignment)
            .await?;
        self.record_snapshot_alignment(snapshot_alignment);
        let published = self.publish_projection_snapshot();
        if published {
            let obligation = self.pending_snapshot_alignment.take();
            self.align_snapshot_if_requested(obligation);
        }
        Ok(delta)
    }

    /// Call a tool by name, returning multimodal content blocks.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let snapshot = Arc::clone(&self.projection);
        let server_name = snapshot
            .tool_to_server
            .get(name)
            .cloned()
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        let entry = self
            .servers
            .get(&server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            }) {
            Ok(transition) => {
                for effect in &transition.effects {
                    if let ExternalToolSurfaceEffect::RejectSurfaceCall { cause, .. } = effect {
                        return Err(McpError::ServerUnavailable {
                            server: server_name.clone(),
                            state: cause.as_str().to_owned(),
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

        let conn = entry
            .connection
            .as_ref()
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        let _guard = InflightCallGuard::new(&entry.active_calls);
        let result = conn.call_tool(name, args).await;

        // Fail closed (matching CallStarted): a rejected CallFinished is
        // authoritative divergence, not a benign no-op. Surface the tool
        // result only when the surface owner accepts the finish.
        if let Err(error) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallFinished {
                surface_id: sid.clone(),
            })
        {
            return Err(McpError::ServerUnavailable {
                server: server_name.clone(),
                state: format!("Surface owner rejected CallFinished: {error}"),
            });
        }

        result
    }

    /// Gracefully shutdown all connections.
    pub async fn shutdown(mut self) {
        // First consume any finished pending tasks through normal processing so
        // stale completion payloads close their transports via existing paths.
        self.drain_pending();
        let _ = self.surface_owner.apply(ExternalToolSurfaceInput::Shutdown);
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
                Self::close_entry_connection(pending.obligation.surface_id, Some(conn)).await;
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
            // Sync owner inflight count to match the shell's test override.
            if count > current {
                for _ in current..count {
                    let _ = self
                        .surface_owner
                        .apply(ExternalToolSurfaceInput::CallStarted {
                            surface_id: sid.clone(),
                        });
                }
            } else {
                for _ in count..current {
                    let _ = self
                        .surface_owner
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

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: true,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.projection_catalog()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.pending_sources_snapshot().into()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        // K1: external dispatch goes through the typed tool-argument
        // contract — malformed / non-object args fail closed instead of
        // being wrapped into a `Value::String` and forwarded.
        let args = meerkat_core::ToolCallArguments::from_raw_json(call.args)
            .map_err(|err| ToolError::invalid_arguments(call.name, err.to_string()))?;
        let blocks = self
            .call_tool(call.name, args.as_value())
            .await
            .map_err(|e| match e {
                McpError::ToolNotFound(name) => ToolError::NotFound { name },
                other => ToolError::ExecutionFailed {
                    message: other.to_string(),
                },
            })?;

        Ok(ToolResult::with_blocks(call.id.to_string(), blocks, false).into())
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        Some(McpRouter::external_tool_surface_snapshot(self))
    }
}

impl Default for McpRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::connection::McpConnection;
    use meerkat_core::ExternalToolSurfaceFailureCause;
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

    fn generated_surface_handle() -> Arc<dyn ExternalToolSurfaceHandle> {
        Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral())
    }

    fn generated_handle_owner_router() -> McpRouter {
        McpRouter::new_with_surface_handle(generated_surface_handle())
    }

    fn generated_handle_owner_router_with_timeout(removal_timeout: Duration) -> McpRouter {
        McpRouter::new_with_surface_handle_and_removal_timeout(
            generated_surface_handle(),
            removal_timeout,
        )
    }

    #[derive(Default)]
    struct RecordingSurfaceHandle {
        inputs: Mutex<Vec<CoreSurfaceInput>>,
    }

    impl RecordingSurfaceHandle {
        fn recorded_inputs(&self) -> Vec<CoreSurfaceInput> {
            self.inputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl ExternalToolSurfaceHandle for RecordingSurfaceHandle {
        fn apply_surface_input(
            &self,
            input: CoreSurfaceInput,
        ) -> Result<CoreSurfaceTransition, DslTransitionError> {
            let effects = match &input {
                CoreSurfaceInput::MarkPendingFailed {
                    surface_id, cause, ..
                } => {
                    vec![CoreSurfaceEffect::EmitExternalToolDelta {
                        surface_id: surface_id.clone(),
                        operation: ExternalToolSurfaceDeltaOperation::Add,
                        phase: ExternalToolSurfaceDeltaPhase::Failed,
                        cause: Some(*cause),
                    }]
                }
                _ => Vec::new(),
            };
            self.inputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(input);
            Ok(CoreSurfaceTransition {
                phase: ExternalToolSurfaceGlobalPhase::Operating,
                effects,
            })
        }

        fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::StageAdd { surface_id, now_ms })
                .map(|_| ())
        }

        fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::StageRemove { surface_id, now_ms })
                .map(|_| ())
        }

        fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::StageReload { surface_id, now_ms })
                .map(|_| ())
        }

        fn apply_boundary(
            &self,
            surface_id: String,
            now_ms: u64,
            staged_intent_sequence: u64,
            applied_at_turn: u64,
        ) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::ApplyBoundary {
                surface_id,
                now_ms,
                staged_intent_sequence,
                applied_at_turn,
            })
            .map(|_| ())
        }

        fn mark_pending_succeeded(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
        ) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::MarkPendingSucceeded {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            })
            .map(|_| ())
        }

        fn mark_pending_failed(
            &self,
            surface_id: String,
            pending_task_sequence: u64,
            staged_intent_sequence: u64,
            cause: ExternalToolSurfaceFailureCause,
        ) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::MarkPendingFailed {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            })
            .map(|_| ())
        }

        fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::CallStarted { surface_id })
                .map(|_| ())
        }

        fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::CallFinished { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalClean { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalForced { surface_id })
                .map(|_| ())
        }

        fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::SnapshotAligned { epoch })
                .map(|_| ())
        }

        fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::Shutdown)
                .map(|_| ())
        }

        fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
            None
        }

        fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
            SurfaceDiagnosticSnapshot {
                surface_phase: ExternalToolSurfaceGlobalPhase::Operating,
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

    #[test]
    fn runtime_owner_pending_failed_preserves_typed_failure_cause() {
        let handle = Arc::new(RecordingSurfaceHandle::default());
        let owner = SurfaceOwner::runtime(handle.clone());

        let transition = owner
            .apply(ExternalToolSurfaceInput::PendingFailed {
                surface_id: SurfaceId::from("typed-failure"),
                operation: SurfaceDeltaOperation::Add,
                pending_task_sequence: 7,
                staged_intent_sequence: 11,
                applied_at_turn: TurnNumber(13),
                cause: ExternalToolSurfaceFailureCause::PendingFailed,
            })
            .expect("runtime pending failure");
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::EmitExternalToolDelta {
                phase: SurfaceDeltaPhase::Failed,
                cause: Some(ExternalToolSurfaceFailureCause::PendingFailed),
                ..
            }
        )));

        let recorded = handle.recorded_inputs();
        assert_eq!(recorded.len(), 1);
        let CoreSurfaceInput::MarkPendingFailed { cause, .. } = recorded[0] else {
            panic!(
                "expected MarkPendingFailed core input, got {:?}",
                recorded[0]
            );
        };
        assert_eq!(cause, ExternalToolSurfaceFailureCause::PendingFailed);
        assert_eq!(cause.as_str(), "pending_failed");
    }

    fn complete_add(router: &mut McpRouter, server_name: &str) {
        let sid = SurfaceId::from(server_name);
        // Read the per-surface sequences from the owner's surface snapshot
        // after each transition rather than `staged_intents_in_order()[0]` +
        // a hardcoded `pending_task_sequence: 1`. The `[0]` form and the
        // hardcoded task sequence only happen to be correct for the FIRST
        // surface; once a second surface is staged they diverge and
        // PendingSucceeded fails its task/lineage sequence guards. The
        // snapshot exposes the authoritative staged/pending/lineage sequences
        // minted by the handle for this surface.
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
            .expect("stage add");
        let staged_sequence = router
            .surface_owner
            .surface_snapshot(server_name)
            .and_then(|snap| snap.staged_intent_sequence)
            .expect("staged intent sequence after stage add");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply add boundary");
        let pending = router
            .surface_owner
            .surface_snapshot(server_name)
            .expect("surface snapshot after apply boundary");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::PendingSucceeded {
                surface_id: sid,
                operation: SurfaceDeltaOperation::Add,
                pending_task_sequence: pending
                    .pending_task_sequence
                    .expect("pending task sequence after apply boundary"),
                staged_intent_sequence: pending
                    .pending_lineage_sequence
                    .expect("pending lineage sequence after apply boundary"),
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("add success");
    }

    #[test]
    fn core_handle_owner_add_apply_success_makes_surface_active_visible() {
        let mut router = generated_handle_owner_router();

        complete_add(&mut router, "runtime-add");

        let snapshot = router.external_tool_surface_snapshot();
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.surface_id == "runtime-add")
            .expect("runtime-add surface");
        assert!(entry.visible);
        assert_eq!(entry.base_state, ExternalToolSurfaceBaseState::Active);
        assert_eq!(entry.pending_op, ExternalToolSurfacePendingOp::None);
    }

    #[test]
    fn core_handle_owner_reload_rejects_before_active_and_accepts_after_active() {
        let mut router = generated_handle_owner_router();
        let sid = SurfaceId::from("runtime-reload");

        assert!(
            router
                .surface_owner
                .apply(ExternalToolSurfaceInput::StageReload {
                    surface_id: sid.clone(),
                })
                .is_err(),
            "reload before active must be rejected by DSL"
        );

        complete_add(&mut router, "runtime-reload");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageReload {
                surface_id: sid.clone(),
            })
            .expect("reload after active");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        let transition = router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid,
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply reload boundary");
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                operation: SurfaceDeltaOperation::Reload,
                ..
            }
        )));
    }

    #[test]
    fn core_handle_owner_remove_finalize_and_call_guards_are_owner_owned() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "runtime-remove");
        let sid = SurfaceId::from("runtime-remove");

        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("first call");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("second call");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallFinished {
                surface_id: sid.clone(),
            })
            .expect("finish one call");
        assert_eq!(router.surface_owner.inflight_call_count(&sid), 1);

        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageRemove {
                surface_id: sid.clone(),
            })
            .expect("stage remove");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply remove boundary");

        let rejected = router
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("call while removing is modeled rejection");
        assert!(rejected.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::RejectSurfaceCall { cause, .. }
                if *cause == ExternalToolSurfaceFailureCause::SurfaceDraining
        )));
        assert_eq!(router.surface_owner.inflight_call_count(&sid), 1);

        assert!(
            router
                .surface_owner
                .apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
                    surface_id: sid.clone(),
                    applied_at_turn: TurnNumber(staged_sequence),
                })
                .is_err(),
            "clean finalize must reject while inflight calls remain"
        );
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id: sid.clone(),
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("forced finalize");
        assert_eq!(
            router.surface_owner.surface_base(&sid),
            SurfaceBaseState::Removed
        );
        assert_eq!(router.surface_owner.inflight_call_count(&sid), 0);
    }

    /// Surface handle decorator that delegates every read/transition to a real
    /// generated handle, but rejects finalize-removal inputs. Used to prove the
    /// router fails closed (does not silently continue with diverged state)
    /// when the surface owner rejects a finalize the router already computed.
    struct RejectFinalizeSurfaceHandle {
        inner: Arc<dyn ExternalToolSurfaceHandle>,
    }

    impl RejectFinalizeSurfaceHandle {
        fn new() -> Self {
            Self {
                inner: generated_surface_handle(),
            }
        }
    }

    impl ExternalToolSurfaceHandle for RejectFinalizeSurfaceHandle {
        fn apply_surface_input(
            &self,
            input: CoreSurfaceInput,
        ) -> Result<CoreSurfaceTransition, DslTransitionError> {
            match input {
                CoreSurfaceInput::FinalizeRemovalClean { .. }
                | CoreSurfaceInput::FinalizeRemovalForced { .. } => {
                    Err(DslTransitionError::guard_rejected(
                        "RejectFinalizeSurfaceHandle",
                        "finalize removal forcibly rejected for test",
                    ))
                }
                other => self.inner.apply_surface_input(other),
            }
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
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalClean { surface_id })
                .map(|_| ())
        }

        fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
            self.apply_surface_input(CoreSurfaceInput::FinalizeRemovalForced { surface_id })
                .map(|_| ())
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

    /// Surface handle decorator that delegates every read/transition to a real
    /// generated handle, but rejects `CallFinished` inputs. Used to prove the
    /// router fails closed when the surface owner rejects the finish for a tool
    /// call whose underlying transport already returned a result (fix #7).
    struct RejectCallFinishedSurfaceHandle {
        inner: Arc<dyn ExternalToolSurfaceHandle>,
    }

    impl RejectCallFinishedSurfaceHandle {
        fn new() -> Self {
            Self {
                inner: generated_surface_handle(),
            }
        }
    }

    impl ExternalToolSurfaceHandle for RejectCallFinishedSurfaceHandle {
        fn apply_surface_input(
            &self,
            input: CoreSurfaceInput,
        ) -> Result<CoreSurfaceTransition, DslTransitionError> {
            match input {
                CoreSurfaceInput::CallFinished { .. } => Err(DslTransitionError::guard_rejected(
                    "RejectCallFinishedSurfaceHandle",
                    "call finished forcibly rejected for test",
                )),
                other => self.inner.apply_surface_input(other),
            }
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
            self.apply_surface_input(CoreSurfaceInput::CallFinished { surface_id })
                .map(|_| ())
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

    /// Gate for fix #7: a surface-owner rejection of `CallFinished` is
    /// authoritative divergence. The router must fail closed (return a typed
    /// `ServerUnavailable` fault) instead of warn-and-returning the tool result
    /// with router/machine inflight-call state diverged. Mirrors the
    /// fail-closed `CallStarted` rejection path.
    #[tokio::test]
    async fn call_finished_rejection_fails_closed_not_returning_result() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router =
            McpRouter::new_with_surface_handle(Arc::new(RejectCallFinishedSurfaceHandle::new()));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        // The underlying transport succeeds, but the surface owner rejects the
        // CallFinished apply; the result must NOT be surfaced.
        let err = router
            .call_tool("echo", &serde_json::json!({"message": "hi"}))
            .await
            .expect_err("CallFinished rejection must fail closed, not return the result");
        assert!(
            matches!(err, McpError::ServerUnavailable { .. }),
            "expected a typed ServerUnavailable fault, got {err:?}"
        );
    }

    /// Gate for dogma row #112: a surface-owner rejection of a finalize-removal
    /// is authoritative divergence. The router must fail closed (return a typed
    /// error) instead of warn-and-continuing with router/machine state diverged.
    #[tokio::test]
    async fn finalize_removal_rejection_fails_closed_not_silently_continue() {
        let mut router =
            McpRouter::new_with_surface_handle(Arc::new(RejectFinalizeSurfaceHandle::new()));
        let sid = SurfaceId::from("reject-finalize");

        // Drive the surface to Active, then to Removing with zero inflight calls
        // so process_removals decides to clean-finalize it.
        complete_add(&mut router, "reject-finalize");
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageRemove {
                surface_id: sid.clone(),
            })
            .expect("stage remove");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply remove boundary");

        assert_eq!(
            router.surface_owner.inflight_call_count(&sid),
            0,
            "surface should have no inflight calls so finalize-clean is attempted"
        );
        assert!(
            router.surface_owner.removing_surfaces().contains(&sid),
            "surface should be in the removing set"
        );

        // process_removals computes the finalized set then asks the surface owner
        // to commit it; the owner rejects. The OLD behavior warned and returned
        // Ok with diverged state — this must now be a typed error.
        let result = router.progress_removals().await;
        let err = result.expect_err(
            "surface-owner rejection of finalize removal must fail closed, not silently continue",
        );
        assert!(
            matches!(err, McpError::ProtocolError { .. }),
            "expected a typed ProtocolError fault, got {err:?}"
        );
    }

    /// Insert a shell `ServerEntry` directly with the given tool names. Used to
    /// stage tool-name overlap between two visible servers without standing up
    /// real MCP connections.
    fn insert_entry_with_tools(router: &mut McpRouter, server_name: &str, tool_names: &[&str]) {
        let tools: Vec<Arc<ToolDef>> = tool_names
            .iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    *name,
                    format!("tool {name} from {server_name}"),
                    serde_json::json!({"type": "object"}),
                ))
            })
            .collect();
        router.servers.insert(
            server_name.to_string(),
            ServerEntry {
                config: McpServerConfig::stdio(
                    server_name,
                    "noop".to_string(),
                    vec![],
                    HashMap::new(),
                ),
                connection: None,
                tools,
                active_calls: AtomicUsize::new(0),
            },
        );
    }

    /// Gate for fixes #6: a tool name exposed by two different servers is
    /// ambiguous and must be dispatchable from NEITHER. The OLD behavior was
    /// last-wins (silently route to an arbitrary owner). The published
    /// projection must exclude the collided name entirely while leaving each
    /// server's non-colliding tools routable.
    #[tokio::test]
    async fn duplicate_tool_name_across_servers_is_excluded_from_projection() {
        let mut router = generated_handle_owner_router();

        // Two active, visible servers.
        complete_add(&mut router, "server-a");
        complete_add(&mut router, "server-b");

        // Both expose "shared"; each also exposes a unique tool.
        insert_entry_with_tools(&mut router, "server-a", &["shared", "only_a"]);
        insert_entry_with_tools(&mut router, "server-b", &["shared", "only_b"]);

        let complete = router.publish_projection_snapshot();
        assert!(
            !complete,
            "a tool-name collision is an incomplete projection (fail closed)"
        );

        let projection = Arc::clone(&router.projection);

        // The colliding name is absent from routing and the catalog.
        assert!(
            !projection.tool_to_server.contains_key("shared"),
            "collided tool name must not be routable from any server"
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .all(|entry| entry.tool.name.as_str() != "shared"),
            "collided tool name must be excluded from the published catalog"
        );

        // Non-colliding tools from each server remain routable.
        assert_eq!(
            projection.tool_to_server.get("only_a").map(String::as_str),
            Some("server-a")
        );
        assert_eq!(
            projection.tool_to_server.get("only_b").map(String::as_str),
            Some("server-b")
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .any(|entry| entry.tool.name.as_str() == "only_a")
        );
        assert!(
            projection
                .catalog_entries
                .iter()
                .any(|entry| entry.tool.name.as_str() == "only_b")
        );

        // Dispatching the ambiguous name fails not_found (denied, never routed).
        let err = router
            .call_tool("shared", &serde_json::json!({}))
            .await
            .expect_err("collided tool name must not dispatch");
        assert!(
            matches!(err, McpError::ToolNotFound(_)),
            "ambiguous tool name must be denied as not_found, got {err:?}"
        );
    }

    /// Same-name-same-server re-entry (the same tool listed twice by one server)
    /// is NOT a collision; the tool stays routable and the projection complete.
    #[test]
    fn duplicate_tool_name_within_single_server_is_not_a_collision() {
        let mut router = generated_handle_owner_router();
        complete_add(&mut router, "solo");
        insert_entry_with_tools(&mut router, "solo", &["dup", "dup", "unique"]);

        let complete = router.publish_projection_snapshot();
        assert!(
            complete,
            "a same-server duplicate is not a collision; projection stays complete"
        );

        let projection = Arc::clone(&router.projection);
        assert_eq!(
            projection.tool_to_server.get("dup").map(String::as_str),
            Some("solo"),
            "same-server duplicate tool name remains routable"
        );
        assert_eq!(
            projection.tool_to_server.get("unique").map(String::as_str),
            Some("solo")
        );
    }

    #[test]
    fn core_handle_owner_uses_owner_pending_operation_over_completion_hint() {
        let router = generated_handle_owner_router();
        let sid = SurfaceId::from("runtime-wrong-op");

        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
            .expect("stage add");
        let staged_sequence = router.surface_owner.staged_intents_in_order()[0].2;
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("apply add boundary");

        let transition = router
            .surface_owner
            .apply(ExternalToolSurfaceInput::PendingSucceeded {
                surface_id: sid.clone(),
                operation: SurfaceDeltaOperation::Reload,
                pending_task_sequence: 1,
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("completion hint must not override machine-owned pending operation");
        assert!(
            transition.effects.iter().any(|effect| matches!(
                effect,
                ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation: SurfaceDeltaOperation::Add,
                    phase: SurfaceDeltaPhase::Applied,
                    ..
                } if *surface_id == sid
            )),
            "machine-owned pending operation should drive emitted delta: {transition:?}"
        );
    }

    #[tokio::test]
    async fn staged_add_remove_reload_transitions() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = generated_handle_owner_router();

        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        assert!(matches!(
            router.server_lifecycle_state("test-server"),
            Some(McpServerLifecycleState::Active)
        ));

        router.stage_reload("test-server").expect("stage reload");
        let result = router.apply_staged().await.expect("apply staged reload");
        assert!(result.pending_count > 0 || !result.delta.reloaded_servers.is_empty());

        tokio::time::sleep(Duration::from_millis(500)).await;
        let ext = router.take_external_updates();
        assert!(
            ext.notices.iter().any(|n| n.target == "test-server")
                || router.server_lifecycle_state("test-server")
                    == Some(McpServerLifecycleState::Active),
        );

        router.stage_remove("test-server").expect("stage remove");
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

        let mut router = generated_handle_owner_router();
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        let has_echo_before = router.list_tools().iter().any(|tool| tool.name == "echo");
        assert!(has_echo_before, "tool should be visible before remove");

        router.stage_remove("test-server").expect("stage remove");
        router.apply_staged().await.expect("apply remove");

        let has_echo_after = router.list_tools().iter().any(|tool| tool.name == "echo");
        assert!(!has_echo_after, "tool should be hidden immediately");
    }

    #[tokio::test]
    async fn removing_state_rejects_new_calls_and_drains_inflight() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = generated_handle_owner_router_with_timeout(Duration::from_secs(60));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        router.set_inflight_calls_for_testing("test-server", 1);
        router.stage_remove("test-server").expect("stage remove");
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

        let mut router = generated_handle_owner_router_with_timeout(Duration::from_millis(10));
        router
            .add_server(test_server_config("test-server", &server_path))
            .await
            .expect("add_server");

        router.set_inflight_calls_for_testing("test-server", 1);
        router.stage_remove("test-server").expect("stage remove");
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

        let mut router = generated_handle_owner_router();
        router
            .stage_add(test_server_config("test-server", &server_path))
            .expect("stage add");
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

        let mut router = generated_handle_owner_router();

        router
            .stage_add(test_server_config("test-server", &server_path))
            .expect("stage add");
        let result = router.apply_staged().await.expect("first add");
        assert_eq!(result.pending_count, 1);

        router.stage_remove("test-server").expect("stage remove");
        router.apply_staged().await.expect("remove");

        router
            .stage_add(test_server_config("test-server", &server_path))
            .expect("stage add");
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
