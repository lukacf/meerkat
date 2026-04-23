//! MCP router for multi-server routing
//!
//! The router is a shell that manages connections, async tasks, and tool
//! caching. Runtime-backed routers treat the shared
//! [`ExternalToolSurfaceHandle`] as the production owner path; standalone
//! routers keep using [`ExternalToolSurfaceAuthority`].

use crate::external_tool_surface_authority::{
    ExternalToolSurfaceAuthority, ExternalToolSurfaceEffect, ExternalToolSurfaceError,
    ExternalToolSurfaceInput, ExternalToolSurfaceMutator, ExternalToolSurfacePhase,
    ExternalToolSurfaceTransition, PendingSurfaceOp, RemovalTimingInfo, StagedSurfaceOp,
    SurfaceBaseState, SurfaceDeltaOperation, SurfaceDeltaPhase, SurfaceId, TurnNumber,
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
use meerkat_core::ToolCatalogCapabilities;
use meerkat_core::ToolCatalogEntry;
use meerkat_core::error::ToolError;
use meerkat_core::event::{ExternalToolDelta, ExternalToolDeltaPhase, ToolConfigChangeOperation};
use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceHandle, McpServerLifecycleHandle,
    SurfaceDiagnosticSnapshot, SurfaceSnapshot,
};
use meerkat_core::tool_catalog::stable_owner_key_for_tool;
use meerkat_core::types::ToolDef;
use meerkat_core::types::{ContentBlock, ToolCallView, ToolResult};
use meerkat_core::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceEntrySnapshot, ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp,
    ExternalToolSurfaceSnapshot, ExternalToolSurfaceStagedOp,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const DEFAULT_REMOVAL_TIMEOUT: Duration = Duration::from_secs(30);
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

enum SurfaceOwner {
    Standalone(Box<Mutex<ExternalToolSurfaceAuthority>>),
    Runtime {
        handle: Arc<dyn ExternalToolSurfaceHandle>,
        removal_timeout_hint: Duration,
    },
}

impl SurfaceOwner {
    fn standalone(authority: ExternalToolSurfaceAuthority) -> Self {
        Self::Standalone(Box::new(Mutex::new(authority)))
    }

    fn runtime(handle: Arc<dyn ExternalToolSurfaceHandle>, removal_timeout_hint: Duration) -> Self {
        Self::Runtime {
            handle,
            removal_timeout_hint,
        }
    }

    fn diagnostic_snapshot(&self) -> ExternalToolSurfaceSnapshot {
        match self {
            Self::Standalone(authority) => authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostic_snapshot(),
            Self::Runtime { handle, .. } => snapshot_from_handle(handle.diagnostic_snapshot()),
        }
    }

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
        match self {
            Self::Standalone(_) => None,
            Self::Runtime { handle, .. } => handle.surface_snapshot(surface_id),
        }
    }

    fn set_removal_timeout(
        &mut self,
        removal_timeout: Duration,
    ) -> Result<(), ExternalToolSurfaceError> {
        match self {
            Self::Standalone(authority) => authority
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_removal_timeout(removal_timeout),
            Self::Runtime {
                removal_timeout_hint,
                ..
            } => {
                *removal_timeout_hint = removal_timeout;
                Ok(())
            }
        }
    }

    fn configured_removal_timeout(&self) -> Duration {
        match self {
            Self::Standalone(_) => DEFAULT_REMOVAL_TIMEOUT,
            Self::Runtime {
                removal_timeout_hint,
                ..
            } => *removal_timeout_hint,
        }
    }

    fn apply(
        &self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        match self {
            Self::Standalone(authority) => authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .apply(input),
            Self::Runtime { handle, .. } => Self::apply_runtime(handle.as_ref(), input),
        }
    }

    fn apply_runtime(
        handle: &dyn ExternalToolSurfaceHandle,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        match input {
            ExternalToolSurfaceInput::StageAdd { surface_id } => {
                handle
                    .stage_add(surface_id.0, McpRouter::now_ms())
                    .map_err(|error| runtime_surface_error("StageAdd", error))?;
                Ok(runtime_transition(
                    "StageAdd",
                    &[],
                    ExternalToolSurfacePhase::Operating,
                ))
            }
            ExternalToolSurfaceInput::StageRemove { surface_id } => {
                handle
                    .stage_remove(surface_id.0, McpRouter::now_ms())
                    .map_err(|error| runtime_surface_error("StageRemove", error))?;
                Ok(runtime_transition(
                    "StageRemove",
                    &[],
                    ExternalToolSurfacePhase::Operating,
                ))
            }
            ExternalToolSurfaceInput::StageReload { surface_id } => {
                handle
                    .stage_reload(surface_id.0, McpRouter::now_ms())
                    .map_err(|error| runtime_surface_error("StageReload", error))?;
                Ok(runtime_transition(
                    "StageReload",
                    &[],
                    ExternalToolSurfacePhase::Operating,
                ))
            }
            ExternalToolSurfaceInput::ApplyBoundary {
                surface_id,
                applied_at_turn,
            } => {
                let before = handle.diagnostic_snapshot();
                handle
                    .apply_boundary(surface_id.0.clone(), McpRouter::now_ms(), applied_at_turn.0)
                    .map_err(|error| runtime_surface_error("ApplyBoundary", error))?;
                let after = handle.diagnostic_snapshot();
                let after_snapshot = snapshot_from_handle(after.clone());
                let after_entry = find_handle_entry(&after, &surface_id.0);
                let mut effects = Vec::new();
                let transition_name = if let Some(entry) = after_entry {
                    match entry.pending_op {
                        ExternalToolSurfacePendingOp::Add => {
                            effects.push(ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                                surface_id: surface_id.clone(),
                                operation: SurfaceDeltaOperation::Add,
                                pending_task_sequence: entry.pending_task_sequence.unwrap_or(0),
                                staged_intent_sequence: entry.pending_lineage_sequence.unwrap_or(0),
                                applied_at_turn,
                            });
                            effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                                surface_id: surface_id.clone(),
                                operation: SurfaceDeltaOperation::Add,
                                phase: SurfaceDeltaPhase::Pending,
                                persisted: false,
                                applied_at_turn,
                            });
                            "ApplyBoundaryAdd"
                        }
                        ExternalToolSurfacePendingOp::Reload => {
                            effects.push(ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                                surface_id: surface_id.clone(),
                                operation: SurfaceDeltaOperation::Reload,
                                pending_task_sequence: entry.pending_task_sequence.unwrap_or(0),
                                staged_intent_sequence: entry.pending_lineage_sequence.unwrap_or(0),
                                applied_at_turn,
                            });
                            effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                                surface_id: surface_id.clone(),
                                operation: SurfaceDeltaOperation::Reload,
                                phase: SurfaceDeltaPhase::Pending,
                                persisted: false,
                                applied_at_turn,
                            });
                            "ApplyBoundaryReload"
                        }
                        ExternalToolSurfacePendingOp::None => {
                            if entry
                                .base_state
                                .unwrap_or(ExternalToolSurfaceBaseState::Absent)
                                == ExternalToolSurfaceBaseState::Removing
                                && entry
                                    .last_delta_phase
                                    .unwrap_or(ExternalToolSurfaceDeltaPhase::None)
                                    == ExternalToolSurfaceDeltaPhase::Draining
                            {
                                if after.snapshot_epoch > before.snapshot_epoch {
                                    effects.push(
                                        ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                                            snapshot_epoch: after.snapshot_epoch,
                                        },
                                    );
                                }
                                effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                                    surface_id: surface_id.clone(),
                                    operation: SurfaceDeltaOperation::Remove,
                                    phase: SurfaceDeltaPhase::Draining,
                                    persisted: false,
                                    applied_at_turn,
                                });
                                "ApplyBoundaryRemoveDraining"
                            } else {
                                "ApplyBoundaryRemoveNoop"
                            }
                        }
                    }
                } else {
                    "ApplyBoundary"
                };
                Ok(runtime_transition(
                    transition_name,
                    &effects,
                    phase_from_snapshot(after_snapshot.phase),
                ))
            }
            ExternalToolSurfaceInput::PendingSucceeded {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => {
                let before = handle.diagnostic_snapshot();
                protocol_surface_completion::submit_pending_succeeded(
                    handle,
                    SurfaceCompletionObligation {
                        surface_id: surface_id.clone(),
                        operation,
                        pending_task_sequence,
                        staged_intent_sequence,
                        applied_at_turn,
                    },
                )
                .map_err(|error| runtime_surface_error("PendingSucceeded", error))?;
                let after = handle.diagnostic_snapshot();
                let after_snapshot = snapshot_from_handle(after);
                let mut effects = Vec::new();
                if after_snapshot.snapshot_epoch > before.snapshot_epoch {
                    effects.push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                        snapshot_epoch: after_snapshot.snapshot_epoch,
                    });
                }
                effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation,
                    phase: SurfaceDeltaPhase::Applied,
                    persisted: true,
                    applied_at_turn,
                });
                Ok(runtime_transition(
                    match operation {
                        SurfaceDeltaOperation::Add => "PendingSucceededAdd",
                        SurfaceDeltaOperation::Reload => "PendingSucceededReload",
                        SurfaceDeltaOperation::Remove | SurfaceDeltaOperation::None => {
                            "PendingSucceeded"
                        }
                    },
                    &effects,
                    phase_from_snapshot(after_snapshot.phase),
                ))
            }
            ExternalToolSurfaceInput::PendingFailed {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => {
                protocol_surface_completion::submit_pending_failed(
                    handle,
                    SurfaceCompletionObligation {
                        surface_id: surface_id.clone(),
                        operation,
                        pending_task_sequence,
                        staged_intent_sequence,
                        applied_at_turn,
                    },
                    "pending_failed",
                )
                .map_err(|error| runtime_surface_error("PendingFailed", error))?;
                let after_snapshot = snapshot_from_handle(handle.diagnostic_snapshot());
                let effects = vec![ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation,
                    phase: SurfaceDeltaPhase::Failed,
                    persisted: true,
                    applied_at_turn,
                }];
                Ok(runtime_transition(
                    match operation {
                        SurfaceDeltaOperation::Add => "PendingFailedAdd",
                        SurfaceDeltaOperation::Reload => "PendingFailedReload",
                        SurfaceDeltaOperation::Remove | SurfaceDeltaOperation::None => {
                            "PendingFailed"
                        }
                    },
                    &effects,
                    phase_from_snapshot(after_snapshot.phase),
                ))
            }
            ExternalToolSurfaceInput::CallStarted { surface_id } => {
                let before_calls = handle
                    .surface_snapshot(&surface_id.0)
                    .map(|entry| entry.inflight_calls)
                    .unwrap_or(0);
                handle
                    .call_started(surface_id.0.clone())
                    .map_err(|error| runtime_surface_error("CallStarted", error))?;
                let after_entry = handle.surface_snapshot(&surface_id.0);
                let after_calls = after_entry
                    .as_ref()
                    .map(|entry| entry.inflight_calls)
                    .unwrap_or(0);
                let after_base = after_entry
                    .as_ref()
                    .and_then(|entry| entry.base_state)
                    .unwrap_or(ExternalToolSurfaceBaseState::Absent);
                let mut effects = Vec::new();
                let transition_name = if after_base == ExternalToolSurfaceBaseState::Active
                    && after_calls == before_calls.saturating_add(1)
                {
                    "CallStartedActive"
                } else {
                    effects.push(ExternalToolSurfaceEffect::RejectSurfaceCall {
                        surface_id,
                        reason: if after_base == ExternalToolSurfaceBaseState::Removing {
                            "surface_draining".to_string()
                        } else {
                            "surface_unavailable".to_string()
                        },
                    });
                    if after_base == ExternalToolSurfaceBaseState::Removing {
                        "CallStartedRejectWhileRemoving"
                    } else {
                        "CallStartedRejectWhileUnavailable"
                    }
                };
                Ok(runtime_transition(
                    transition_name,
                    &effects,
                    ExternalToolSurfacePhase::Operating,
                ))
            }
            ExternalToolSurfaceInput::CallFinished { surface_id } => {
                handle
                    .call_finished(surface_id.0)
                    .map_err(|error| runtime_surface_error("CallFinished", error))?;
                Ok(runtime_transition(
                    "CallFinished",
                    &[],
                    ExternalToolSurfacePhase::Operating,
                ))
            }
            ExternalToolSurfaceInput::FinalizeRemovalClean {
                surface_id,
                applied_at_turn,
            } => {
                let before = handle.diagnostic_snapshot();
                handle
                    .finalize_removal_clean(surface_id.0.clone())
                    .map_err(|error| runtime_surface_error("FinalizeRemovalClean", error))?;
                let after_snapshot = snapshot_from_handle(handle.diagnostic_snapshot());
                let mut effects = vec![ExternalToolSurfaceEffect::CloseSurfaceConnection {
                    surface_id: surface_id.clone(),
                }];
                if after_snapshot.snapshot_epoch > before.snapshot_epoch {
                    effects.push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                        snapshot_epoch: after_snapshot.snapshot_epoch,
                    });
                }
                effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation: SurfaceDeltaOperation::Remove,
                    phase: SurfaceDeltaPhase::Applied,
                    persisted: true,
                    applied_at_turn,
                });
                Ok(runtime_transition(
                    "FinalizeRemovalClean",
                    &effects,
                    phase_from_snapshot(after_snapshot.phase),
                ))
            }
            ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id,
                applied_at_turn,
            } => {
                let before = handle.diagnostic_snapshot();
                handle
                    .finalize_removal_forced(surface_id.0.clone())
                    .map_err(|error| runtime_surface_error("FinalizeRemovalForced", error))?;
                let after_snapshot = snapshot_from_handle(handle.diagnostic_snapshot());
                let mut effects = vec![ExternalToolSurfaceEffect::CloseSurfaceConnection {
                    surface_id: surface_id.clone(),
                }];
                if after_snapshot.snapshot_epoch > before.snapshot_epoch {
                    effects.push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                        snapshot_epoch: after_snapshot.snapshot_epoch,
                    });
                }
                effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id,
                    operation: SurfaceDeltaOperation::Remove,
                    phase: SurfaceDeltaPhase::Forced,
                    persisted: true,
                    applied_at_turn,
                });
                Ok(runtime_transition(
                    "FinalizeRemovalForced",
                    &effects,
                    phase_from_snapshot(after_snapshot.phase),
                ))
            }
            ExternalToolSurfaceInput::SnapshotAligned { snapshot_epoch } => {
                handle
                    .snapshot_aligned(snapshot_epoch)
                    .map_err(|error| runtime_surface_error("SnapshotAligned", error))?;
                Ok(runtime_transition(
                    "SnapshotAligned",
                    &[],
                    ExternalToolSurfacePhase::Operating,
                ))
            }
            ExternalToolSurfaceInput::Shutdown => {
                handle
                    .shutdown_surface()
                    .map_err(|error| runtime_surface_error("Shutdown", error))?;
                Ok(runtime_transition(
                    "Shutdown",
                    &[],
                    ExternalToolSurfacePhase::Shutdown,
                ))
            }
        }
    }

    fn staged_intents_in_order(&self) -> Vec<(SurfaceId, StagedSurfaceOp, u64)> {
        match self {
            Self::Standalone(authority) => authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .staged_intents_in_order(),
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

    fn pending_op(&self, id: &SurfaceId) -> PendingSurfaceOp {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| pending_surface_op_from_snapshot(entry.pending_op))
            .unwrap_or(PendingSurfaceOp::None)
    }

    fn pending_task_sequence(&self, id: &SurfaceId) -> u64 {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| entry.pending_task_sequence)
            .unwrap_or(0)
    }

    fn pending_lineage_sequence(&self, id: &SurfaceId) -> u64 {
        self.diagnostic_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.surface_id == id.0)
            .map(|entry| entry.pending_lineage_sequence)
            .unwrap_or(0)
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
            Self::Standalone(authority) => authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .removal_timing(id),
            Self::Runtime {
                removal_timeout_hint,
                ..
            } => {
                let entry = self.surface_snapshot(&id.0)?;
                let draining_since_ms = entry.removal_draining_since_ms?;
                let timeout_at_ms = draining_since_ms.saturating_add(
                    (*removal_timeout_hint)
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                );
                Some(RemovalTimingInfo {
                    draining_since: instant_from_epoch_ms(draining_since_ms),
                    timeout_at: instant_from_epoch_ms(timeout_at_ms),
                    applied_at_turn: TurnNumber(entry.removal_applied_at_turn.unwrap_or(0)),
                })
            }
        }
    }
}

fn runtime_transition(
    transition_name: &str,
    effects: &[ExternalToolSurfaceEffect],
    phase: ExternalToolSurfacePhase,
) -> ExternalToolSurfaceTransition {
    ExternalToolSurfaceTransition {
        transition_name: transition_name.to_string(),
        phase,
        effects: effects.to_vec(),
    }
}

fn runtime_surface_error(input_name: &str, error: DslTransitionError) -> ExternalToolSurfaceError {
    ExternalToolSurfaceError {
        input_name: input_name.to_string(),
        reason: error.to_string(),
    }
}

fn find_handle_entry<'a>(
    snapshot: &'a SurfaceDiagnosticSnapshot,
    surface_id: &str,
) -> Option<&'a SurfaceSnapshot> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.surface_id == surface_id)
}

fn phase_from_snapshot(phase: ExternalToolSurfaceGlobalPhase) -> ExternalToolSurfacePhase {
    match phase {
        ExternalToolSurfaceGlobalPhase::Operating => ExternalToolSurfacePhase::Operating,
        ExternalToolSurfaceGlobalPhase::Shutdown => ExternalToolSurfacePhase::Shutdown,
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

fn pending_surface_op_from_snapshot(op: ExternalToolSurfacePendingOp) -> PendingSurfaceOp {
    match op {
        ExternalToolSurfacePendingOp::None => PendingSurfaceOp::None,
        ExternalToolSurfacePendingOp::Add => PendingSurfaceOp::Add,
        ExternalToolSurfacePendingOp::Reload => PendingSurfaceOp::Reload,
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
    let mut latest = None;
    for obligation in protocol_surface_snapshot_alignment::extract_obligations(effects) {
        merge_snapshot_alignment(&mut latest, obligation);
    }
    latest
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
/// Runtime-backed routers use the shared runtime handle directly; standalone
/// routers use an in-process [`ExternalToolSurfaceAuthority`]. The router
/// manages connections, async tasks, tool caching, and effect execution.
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

    fn with_lifecycle_handle<F>(&self, f: F)
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
        if let Some(handle) = guard.as_deref()
            && let Err(error) = f(handle)
        {
            tracing::debug!(
                error = %error,
                "McpServerLifecycleHandle DSL apply rejected"
            );
        }
    }

    fn notify_lifecycle_connect_pending(&self, server_name: &str) {
        self.with_lifecycle_handle(|handle| handle.apply_connect_pending(server_name));
    }

    fn notify_lifecycle_connected(&self, server_name: &str) {
        self.with_lifecycle_handle(|handle| handle.apply_connected(server_name));
    }

    fn notify_lifecycle_failed(&self, server_name: &str, failure: &str) {
        self.with_lifecycle_handle(|handle| handle.apply_failed(server_name, failure));
    }

    fn notify_lifecycle_disconnected(&self, server_name: &str) {
        self.with_lifecycle_handle(|handle| handle.apply_disconnected(server_name));
    }

    fn notify_lifecycle_reload(&self, server_name: &str) {
        self.with_lifecycle_handle(|handle| handle.apply_reload(server_name));
    }

    /// Create a new empty router
    pub fn new() -> Self {
        Self::with_surface_owner(SurfaceOwner::standalone(ExternalToolSurfaceAuthority::new()))
    }

    /// Create a new empty router with a runtime-backed surface handle.
    pub fn new_with_surface_handle(surface_handle: Arc<dyn ExternalToolSurfaceHandle>) -> Self {
        Self::with_surface_owner(SurfaceOwner::runtime(
            surface_handle,
            DEFAULT_REMOVAL_TIMEOUT,
        ))
    }

    /// Create a new router with a custom remove-drain timeout.
    pub fn new_with_removal_timeout(removal_timeout: Duration) -> Self {
        Self::with_surface_owner(SurfaceOwner::standalone(
            ExternalToolSurfaceAuthority::with_removal_timeout(removal_timeout),
        ))
    }

    /// Create a new router with a custom remove-drain timeout and runtime-backed surface handle.
    pub fn new_with_surface_handle_and_removal_timeout(
        surface_handle: Arc<dyn ExternalToolSurfaceHandle>,
        removal_timeout: Duration,
    ) -> Self {
        Self::with_surface_owner(SurfaceOwner::runtime(surface_handle, removal_timeout))
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
    pub fn set_removal_timeout(&mut self, removal_timeout: Duration) {
        if let Err(error) = self.surface_owner.set_removal_timeout(removal_timeout) {
            tracing::warn!(
                timeout_ms = removal_timeout.as_millis(),
                error = %error,
                "Surface owner rejected set_removal_timeout"
            );
        }
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
    pub fn stage_add(&mut self, config: McpServerConfig) {
        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd { surface_id: sid })
        {
            Ok(_) => {
                self.notify_lifecycle_connect_pending(&server_name);
                self.staged_payloads.insert(server_name, config);
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageAdd"
                );
            }
        }
    }

    /// Stage a server remove intent for the next boundary apply.
    ///
    /// This only records intent. Removal lifecycle starts on `apply_staged`.
    pub fn stage_remove(&mut self, server_name: impl Into<String>) {
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
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageReload { surface_id: sid })
        {
            Ok(_) => {
                self.notify_lifecycle_reload(&server_name);
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
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageReload"
                );
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
                    self.notify_lifecycle_disconnected(&surface_id.0);
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
        let owner_pending_matches = self.surface_owner.pending_op(&sid) == expected_pending_op;
        let owner_task_matches =
            self.surface_owner.pending_task_sequence(&sid) == obligation.pending_task_sequence;
        let owner_lineage_matches =
            self.surface_owner.pending_lineage_sequence(&sid) == obligation.staged_intent_sequence;

        if !(obligation_matches
            && owner_pending_matches
            && owner_task_matches
            && owner_lineage_matches)
        {
            tracing::warn!(
                server = %server_name,
                operation = ?obligation.operation,
                obligation_matches,
                owner_pending_matches,
                owner_task_matches,
                owner_lineage_matches,
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
                self.notify_lifecycle_connected(&server_name);

                match self
                    .surface_owner
                    .apply(ExternalToolSurfaceInput::PendingSucceeded {
                        surface_id: pending_state.surface_id.clone(),
                        operation: pending_state.operation,
                        pending_task_sequence: pending_state.pending_task_sequence,
                        staged_intent_sequence: pending_state.staged_intent_sequence,
                        applied_at_turn: pending_state.applied_at_turn,
                    }) {
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
                let operation = match pending_state.operation {
                    SurfaceDeltaOperation::Add => ToolConfigChangeOperation::Add,
                    SurfaceDeltaOperation::Reload => ToolConfigChangeOperation::Reload,
                    SurfaceDeltaOperation::Remove | SurfaceDeltaOperation::None => unreachable!(),
                };
                self.notify_lifecycle_failed(&server_name, &err.to_string());

                let snapshot_alignment =
                    match self
                        .surface_owner
                        .apply(ExternalToolSurfaceInput::PendingFailed {
                            surface_id: pending_state.surface_id.clone(),
                            operation: pending_state.operation,
                            pending_task_sequence: pending_state.pending_task_sequence,
                            staged_intent_sequence: pending_state.staged_intent_sequence,
                            applied_at_turn: pending_state.applied_at_turn,
                        }) {
                        Ok(transition) => latest_snapshot_alignment(&transition.effects),
                        Err(e) => {
                            tracing::warn!(
                                server = %server_name,
                                error = %e,
                                "Surface owner rejected PendingFailed"
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
            background_completions: Vec::new(),
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
        let (conn, tools) = McpConnection::connect_and_enumerate(&config).await?;

        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());
        let mut snapshot_alignment = None;

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
                    applied_at_turn,
                }) {
                Ok(t) => t.effects,
                Err(e) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "Surface owner rejected ApplyBoundary in install path"
                    );
                    vec![]
                }
            };
        // Extract and immediately consume the obligation for the synchronous install path.
        let obligations = protocol_surface_completion::extract_obligations(&boundary_effects);
        for obligation in obligations {
            match self
                .surface_owner
                .apply(ExternalToolSurfaceInput::PendingSucceeded {
                    surface_id: obligation.surface_id.clone(),
                    operation: obligation.operation,
                    pending_task_sequence: obligation.pending_task_sequence,
                    staged_intent_sequence: obligation.staged_intent_sequence,
                    applied_at_turn: obligation.applied_at_turn,
                }) {
                Ok(transition) => {
                    if let Some(obligation) = latest_snapshot_alignment(&transition.effects) {
                        merge_snapshot_alignment(&mut snapshot_alignment, obligation);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        server = %server_name,
                        error = %e,
                        "Surface owner rejected PendingSucceeded in install path"
                    );
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

    /// Process removal finalization based on owner-owned timeout tracking.
    async fn process_removals(
        &mut self,
        delta: &mut McpApplyDelta,
        snapshot_alignment: &mut Option<SurfaceSnapshotAlignmentObligation>,
    ) {
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
            match self.surface_owner.apply(input) {
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
                        "Surface owner rejected finalize removal"
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
                canonical_tools.insert(tool.name.clone(), Arc::clone(tool));
            }
        }

        let catalog_entries: Arc<[ToolCatalogEntry]> = canonical_tools
            .values()
            .map(|tool| {
                if let Some(stable_owner_key) = stable_owner_key_for_tool(tool) {
                    ToolCatalogEntry::session_deferred(Arc::clone(tool), true, stable_owner_key)
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
                        // Defensive fallback: the owner should always have timing
                        // for Removing surfaces, but synthesize if missing.
                        let now = Instant::now();
                        Some(McpServerLifecycleState::Removing {
                            draining_since: now,
                            timeout_at: now + self.surface_owner.configured_removal_timeout(),
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
                    if let ExternalToolSurfaceEffect::RejectSurfaceCall { reason, .. } = effect {
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

        let conn = entry
            .connection
            .as_ref()
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        let _guard = InflightCallGuard::new(&entry.active_calls);
        let result = conn.call_tool(name, args).await;

        if let Err(error) = self
            .surface_owner
            .apply(ExternalToolSurfaceInput::CallFinished {
                surface_id: sid.clone(),
            })
        {
            tracing::warn!(
                server = %server_name,
                error = %error,
                "Surface owner rejected CallFinished"
            );
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
