//! MCP router for multi-server routing
//!
//! The router is a shell that manages connections, async tasks, and tool
//! caching. Runtime/session construction injects a runtime-owned
//! [`ExternalToolSurfaceHandle`] as the surface owner. Standalone construction
//! uses a local compatibility handle backed by [`ExternalToolSurfaceAuthority`]
//! until a runtime-owned handle is bound.

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
    DslTransitionError, ExternalToolSurfaceEffect as CoreSurfaceEffect, ExternalToolSurfaceHandle,
    ExternalToolSurfaceInput as CoreSurfaceInput,
    ExternalToolSurfaceTransition as CoreSurfaceTransition, McpServerLifecycleHandle,
    SurfaceDiagnosticSnapshot, SurfaceSnapshot,
};
use meerkat_core::tool_catalog::stable_owner_key_for_tool;
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

/// Local compatibility implementation of the core surface-handle contract.
///
/// Runtime/session wiring should replace this with the runtime-owned
/// MeerkatMachine handle. Keeping this adapter in the MCP crate lets standalone
/// callers and tests use the core trait boundary without depending on
/// the runtime crate.
#[allow(dead_code)]
pub(crate) struct CompatExternalToolSurfaceHandle {
    authority: Mutex<ExternalToolSurfaceAuthority>,
}

#[allow(dead_code)]
impl CompatExternalToolSurfaceHandle {
    pub(crate) fn new(removal_timeout: Duration) -> Self {
        Self {
            authority: Mutex::new(ExternalToolSurfaceAuthority::with_removal_timeout(
                removal_timeout,
            )),
        }
    }

    fn apply_local(
        &self,
        context: &'static str,
        input: ExternalToolSurfaceInput,
    ) -> Result<CoreSurfaceTransition, DslTransitionError> {
        let mut authority = self
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition = authority
            .apply(input)
            .map_err(|error| compat_surface_error(context, error))?;
        Ok(CoreSurfaceTransition {
            phase: core_phase_from_local(transition.phase),
            effects: core_surface_effects(&transition.effects),
        })
    }

    fn with_authority<R>(&self, f: impl FnOnce(&ExternalToolSurfaceAuthority) -> R) -> R {
        let authority = self
            .authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&authority)
    }

    fn local_pending_operation(
        authority: &ExternalToolSurfaceAuthority,
        surface_id: &SurfaceId,
    ) -> SurfaceDeltaOperation {
        match authority.pending_op(surface_id) {
            PendingSurfaceOp::Add => SurfaceDeltaOperation::Add,
            PendingSurfaceOp::Reload => SurfaceDeltaOperation::Reload,
            PendingSurfaceOp::None => SurfaceDeltaOperation::None,
        }
    }
}

impl ExternalToolSurfaceHandle for CompatExternalToolSurfaceHandle {
    fn apply_surface_input(
        &self,
        input: CoreSurfaceInput,
    ) -> Result<CoreSurfaceTransition, DslTransitionError> {
        match input {
            CoreSurfaceInput::StageAdd { surface_id, .. } => self.apply_local(
                "CompatExternalToolSurfaceHandle::stage_add",
                ExternalToolSurfaceInput::StageAdd {
                    surface_id: SurfaceId(surface_id),
                },
            ),
            CoreSurfaceInput::StageRemove { surface_id, .. } => self.apply_local(
                "CompatExternalToolSurfaceHandle::stage_remove",
                ExternalToolSurfaceInput::StageRemove {
                    surface_id: SurfaceId(surface_id),
                },
            ),
            CoreSurfaceInput::StageReload { surface_id, .. } => self.apply_local(
                "CompatExternalToolSurfaceHandle::stage_reload",
                ExternalToolSurfaceInput::StageReload {
                    surface_id: SurfaceId(surface_id),
                },
            ),
            CoreSurfaceInput::ApplyBoundary {
                surface_id,
                staged_intent_sequence,
                applied_at_turn,
                ..
            } => self.apply_local(
                "CompatExternalToolSurfaceHandle::apply_boundary",
                ExternalToolSurfaceInput::ApplyBoundary {
                    surface_id: SurfaceId(surface_id),
                    staged_intent_sequence,
                    applied_at_turn: TurnNumber(applied_at_turn),
                },
            ),
            CoreSurfaceInput::MarkPendingSucceeded {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            } => {
                let mut authority = self
                    .authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let surface_id = SurfaceId(surface_id);
                let operation = Self::local_pending_operation(&authority, &surface_id);
                let transition = authority
                    .apply(ExternalToolSurfaceInput::PendingSucceeded {
                        surface_id,
                        operation,
                        pending_task_sequence,
                        staged_intent_sequence,
                        applied_at_turn: TurnNumber(staged_intent_sequence),
                    })
                    .map_err(|error| {
                        compat_surface_error(
                            "CompatExternalToolSurfaceHandle::mark_pending_succeeded",
                            error,
                        )
                    })?;
                Ok(CoreSurfaceTransition {
                    phase: core_phase_from_local(transition.phase),
                    effects: core_surface_effects(&transition.effects),
                })
            }
            CoreSurfaceInput::MarkPendingFailed {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            } => {
                let mut authority = self
                    .authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let surface_id = SurfaceId(surface_id);
                let operation = Self::local_pending_operation(&authority, &surface_id);
                let transition = authority
                    .apply(ExternalToolSurfaceInput::PendingFailed {
                        surface_id,
                        operation,
                        pending_task_sequence,
                        staged_intent_sequence,
                        applied_at_turn: TurnNumber(staged_intent_sequence),
                        cause,
                    })
                    .map_err(|error| {
                        compat_surface_error(
                            "CompatExternalToolSurfaceHandle::mark_pending_failed",
                            error,
                        )
                    })?;
                Ok(CoreSurfaceTransition {
                    phase: core_phase_from_local(transition.phase),
                    effects: core_surface_effects(&transition.effects),
                })
            }
            CoreSurfaceInput::CallStarted { surface_id } => self.apply_local(
                "CompatExternalToolSurfaceHandle::call_started",
                ExternalToolSurfaceInput::CallStarted {
                    surface_id: SurfaceId(surface_id),
                },
            ),
            CoreSurfaceInput::CallFinished { surface_id } => self.apply_local(
                "CompatExternalToolSurfaceHandle::call_finished",
                ExternalToolSurfaceInput::CallFinished {
                    surface_id: SurfaceId(surface_id),
                },
            ),
            CoreSurfaceInput::FinalizeRemovalClean { surface_id } => self.apply_local(
                "CompatExternalToolSurfaceHandle::finalize_removal_clean",
                ExternalToolSurfaceInput::FinalizeRemovalClean {
                    surface_id: SurfaceId(surface_id),
                    applied_at_turn: TurnNumber(0),
                },
            ),
            CoreSurfaceInput::FinalizeRemovalForced { surface_id } => self.apply_local(
                "CompatExternalToolSurfaceHandle::finalize_removal_forced",
                ExternalToolSurfaceInput::FinalizeRemovalForced {
                    surface_id: SurfaceId(surface_id),
                    applied_at_turn: TurnNumber(0),
                },
            ),
            CoreSurfaceInput::SnapshotAligned { epoch } => self.apply_local(
                "CompatExternalToolSurfaceHandle::snapshot_aligned",
                ExternalToolSurfaceInput::SnapshotAligned {
                    snapshot_epoch: epoch,
                },
            ),
            CoreSurfaceInput::Shutdown => self.apply_local(
                "CompatExternalToolSurfaceHandle::shutdown_surface",
                ExternalToolSurfaceInput::Shutdown,
            ),
        }
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

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
        self.with_authority(|authority| {
            let snapshot = authority.diagnostic_snapshot();
            snapshot
                .entries
                .into_iter()
                .find(|entry| entry.surface_id == surface_id)
                .map(|entry| handle_snapshot_from_local_entry(entry, authority))
        })
    }

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
        self.with_authority(handle_snapshot_from_local_authority)
    }

    fn visible_surfaces(&self) -> BTreeSet<String> {
        self.with_authority(|authority| {
            authority
                .visible_surfaces()
                .map(|surface_id| surface_id.0.clone())
                .collect()
        })
    }

    fn removing_surfaces(&self) -> BTreeSet<String> {
        self.with_authority(|authority| {
            authority
                .removing_surfaces()
                .map(|surface_id| surface_id.0.clone())
                .collect()
        })
    }

    fn pending_surfaces(&self) -> BTreeSet<String> {
        self.with_authority(|authority| {
            authority
                .pending_surfaces()
                .map(|surface_id| surface_id.0.clone())
                .collect()
        })
    }

    fn has_pending_or_staged(&self) -> bool {
        self.with_authority(ExternalToolSurfaceAuthority::has_pending_or_staged)
    }

    fn snapshot_epoch(&self) -> u64 {
        self.with_authority(ExternalToolSurfaceAuthority::snapshot_epoch)
    }

    fn snapshot_aligned_epoch(&self) -> u64 {
        self.with_authority(ExternalToolSurfaceAuthority::snapshot_aligned_epoch)
    }
}

enum SurfaceOwner {
    #[allow(dead_code)]
    CompatStandalone(Box<Mutex<ExternalToolSurfaceAuthority>>),
    Runtime {
        handle_slot: Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>,
        removal_timeout_hint: Duration,
    },
}

impl SurfaceOwner {
    #[allow(dead_code)]
    fn compat_standalone(authority: ExternalToolSurfaceAuthority) -> Self {
        Self::CompatStandalone(Box::new(Mutex::new(authority)))
    }

    fn runtime(handle: Arc<dyn ExternalToolSurfaceHandle>, removal_timeout_hint: Duration) -> Self {
        Self::Runtime {
            handle_slot: Arc::new(StdRwLock::new(handle)),
            removal_timeout_hint,
        }
    }

    fn runtime_handle_slot(&self) -> Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>> {
        match self {
            Self::Runtime { handle_slot, .. } => Some(Arc::clone(handle_slot)),
            Self::CompatStandalone(_) => None,
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
            Self::CompatStandalone(_) => None,
        }
    }

    fn diagnostic_snapshot(&self) -> ExternalToolSurfaceSnapshot {
        match self {
            Self::CompatStandalone(authority) => authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostic_snapshot(),
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
            Self::CompatStandalone(_) => None,
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
            Self::CompatStandalone(authority) => authority
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
            Self::CompatStandalone(_) => DEFAULT_REMOVAL_TIMEOUT,
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
            Self::CompatStandalone(authority) => authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .apply(input),
            Self::Runtime { handle_slot, .. } => {
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
            Self::CompatStandalone(authority) => authority
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
            Self::CompatStandalone(authority) => authority
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

#[allow(dead_code)]
fn compat_surface_error(
    context: &'static str,
    error: ExternalToolSurfaceError,
) -> DslTransitionError {
    DslTransitionError::guard_rejected(context, error.to_string())
}

#[allow(dead_code)]
fn core_phase_from_local(phase: ExternalToolSurfacePhase) -> ExternalToolSurfaceGlobalPhase {
    match phase {
        ExternalToolSurfacePhase::Operating => ExternalToolSurfaceGlobalPhase::Operating,
        ExternalToolSurfacePhase::Shutdown => ExternalToolSurfaceGlobalPhase::Shutdown,
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

#[allow(dead_code)]
fn epoch_ms_from_instant(instant: Instant) -> u64 {
    let now_ms = McpRouter::now_ms();
    let now = Instant::now();
    if instant >= now {
        let delta = instant.duration_since(now);
        now_ms.saturating_add(delta.as_millis().min(u128::from(u64::MAX)) as u64)
    } else {
        let delta = now.duration_since(instant);
        now_ms.saturating_sub(delta.as_millis().min(u128::from(u64::MAX)) as u64)
    }
}

#[allow(dead_code)]
fn handle_snapshot_from_local_authority(
    authority: &ExternalToolSurfaceAuthority,
) -> SurfaceDiagnosticSnapshot {
    let snapshot = authority.diagnostic_snapshot();
    let ExternalToolSurfaceSnapshot {
        phase,
        snapshot_epoch,
        snapshot_aligned_epoch,
        entries,
    } = snapshot;
    let known_surfaces = entries
        .iter()
        .map(|entry| entry.surface_id.clone())
        .collect::<BTreeSet<_>>();
    let visible_surfaces = entries
        .iter()
        .filter(|entry| entry.visible)
        .map(|entry| entry.surface_id.clone())
        .collect::<BTreeSet<_>>();
    let has_pending_or_staged = entries.iter().any(|entry| {
        entry.pending_op != ExternalToolSurfacePendingOp::None
            || entry.staged_op != ExternalToolSurfaceStagedOp::None
    });
    let entries = entries
        .into_iter()
        .map(|entry| handle_snapshot_from_local_entry(entry, authority))
        .collect();

    SurfaceDiagnosticSnapshot {
        surface_phase: phase,
        known_surfaces,
        visible_surfaces,
        snapshot_epoch,
        snapshot_aligned_epoch,
        has_pending_or_staged,
        entries,
    }
}

#[allow(dead_code)]
fn handle_snapshot_from_local_entry(
    entry: ExternalToolSurfaceEntrySnapshot,
    authority: &ExternalToolSurfaceAuthority,
) -> SurfaceSnapshot {
    let timing = authority.removal_timing(&SurfaceId::from(entry.surface_id.as_str()));
    SurfaceSnapshot {
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
        removal_draining_since_ms: timing
            .map(|timing| epoch_ms_from_instant(timing.draining_since)),
        removal_timeout_at_ms: timing.map(|timing| epoch_ms_from_instant(timing.timeout_at)),
        removal_applied_at_turn: timing.map(|timing| timing.applied_at_turn.0),
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

    /// Return the shared runtime surface-handle slot. Runtime-backed session
    /// builds replace the router's default ephemeral DSL owner with the
    /// session-owned MeerkatMachine handle through this slot.
    pub fn external_surface_handle_slot(
        &self,
    ) -> Option<Arc<StdRwLock<Arc<dyn ExternalToolSurfaceHandle>>>> {
        self.surface_owner.runtime_handle_slot()
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

    /// Create a new empty standalone router with a local surface authority.
    ///
    /// Production/session construction must bind a runtime-owned
    /// [`ExternalToolSurfaceHandle`] through [`Self::new_with_surface_handle`] or
    /// late adapter binding when lifecycle state must be session-owned.
    /// Standalone callers keep a local compatibility handle so lifecycle
    /// operations work outside a `MeerkatMachine`.
    pub fn new() -> Self {
        let surface_handle: Arc<dyn ExternalToolSurfaceHandle> = Arc::new(
            CompatExternalToolSurfaceHandle::new(DEFAULT_REMOVAL_TIMEOUT),
        );
        Self::with_surface_owner(SurfaceOwner::runtime(
            surface_handle,
            DEFAULT_REMOVAL_TIMEOUT,
        ))
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
        let surface_handle: Arc<dyn ExternalToolSurfaceHandle> =
            Arc::new(CompatExternalToolSurfaceHandle::new(removal_timeout));
        Self::with_surface_owner(SurfaceOwner::runtime(surface_handle, removal_timeout))
    }

    /// Test-only compatibility constructor for the retired standalone
    /// handwritten surface authority.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn new_with_compat_standalone_authority_for_testing() -> Self {
        Self::with_surface_owner(SurfaceOwner::compat_standalone(
            ExternalToolSurfaceAuthority::new(),
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
    pub fn stage_add(&mut self, config: McpServerConfig) -> Result<(), ExternalToolSurfaceError> {
        let server_name = config.name.clone();
        let sid = SurfaceId::from(server_name.as_str());
        match self
            .surface_owner
            .apply(ExternalToolSurfaceInput::StageAdd { surface_id: sid })
        {
            Ok(_) => {
                self.notify_lifecycle_connect_pending(&server_name);
                self.staged_payloads.insert(server_name, config);
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageAdd"
                );
                Err(error)
            }
        }
    }

    /// Stage a server remove intent for the next boundary apply.
    ///
    /// This only records intent. Removal lifecycle starts on `apply_staged`.
    pub fn stage_remove(
        &mut self,
        server_name: impl Into<String>,
    ) -> Result<(), ExternalToolSurfaceError> {
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
            return Err(error);
        }
        self.staged_payloads.remove(&server_name);
        Ok(())
    }

    /// Stage a server reload by server name (reuse existing config) or full config.
    pub fn stage_reload<T: Into<McpReloadTarget>>(
        &mut self,
        target: T,
    ) -> Result<(), ExternalToolSurfaceError> {
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
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    error = %error,
                    "Surface owner rejected StageReload"
                );
                Err(error)
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
                        self.notify_lifecycle_connected(&server_name);
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
                self.notify_lifecycle_failed(&server_name, &err.to_string());

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
                        return None;
                    }
                };

                tracing::warn!(
                    server = %server_name,
                    error = %err,
                    op = ?obligation.operation,
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
            if let Err(close_error) = conn.close().await {
                tracing::debug!(
                    server = %server_name,
                    error = %close_error,
                    "Error closing MCP connection after rejected StageAdd"
                );
            }
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
                    if let Err(close_error) = conn.close().await {
                        tracing::debug!(
                            server = %server_name,
                            error = %close_error,
                            "Error closing MCP connection after rejected ApplyBoundary"
                        );
                    }
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
                if let Err(close_error) = conn.close().await {
                    tracing::debug!(
                        server = %server_name,
                        error = %close_error,
                        "Error closing MCP connection after missing surface completion obligation"
                    );
                }
                return Err(McpError::ProtocolError {
                    message: "surface owner ApplyBoundary emitted no surface completion obligation"
                        .into(),
                });
            }
        };
        let mut applied_operation = ToolConfigChangeOperation::Add;
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
                if let Some(operation) = lifecycle_operation_from_effects(
                    &transition.effects,
                    SurfaceDeltaPhase::Applied,
                ) {
                    applied_operation = operation;
                }
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
                if let Err(close_error) = conn.close().await {
                    tracing::debug!(
                        server = %server_name,
                        error = %close_error,
                        "Error closing MCP connection after rejected PendingSucceeded"
                    );
                }
                return Err(McpError::ProtocolError {
                    message: format!("surface owner rejected PendingSucceeded: {e}"),
                });
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
                applied_operation,
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
                    tool_to_server.insert(tool.name.to_string(), server_name.clone())
                    && previous_owner != server_name
                {
                    tracing::warn!(
                        tool = %tool.name,
                        previous_owner = %previous_owner,
                        current_owner = %server_name,
                        "MCP projection remapped duplicate tool name to newer owner"
                    );
                }
                canonical_tools.insert(tool.name.to_string(), Arc::clone(tool));
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

    fn compat_handle_owner_router() -> McpRouter {
        McpRouter::new_with_surface_handle(Arc::new(CompatExternalToolSurfaceHandle::new(
            DEFAULT_REMOVAL_TIMEOUT,
        )))
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
        let owner = SurfaceOwner::runtime(handle.clone(), DEFAULT_REMOVAL_TIMEOUT);

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
        router
            .surface_owner
            .apply(ExternalToolSurfaceInput::PendingSucceeded {
                surface_id: sid,
                operation: SurfaceDeltaOperation::Add,
                pending_task_sequence: 1,
                staged_intent_sequence: staged_sequence,
                applied_at_turn: TurnNumber(staged_sequence),
            })
            .expect("add success");
    }

    #[test]
    fn core_handle_owner_add_apply_success_makes_surface_active_visible() {
        let mut router = compat_handle_owner_router();

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
        let mut router = compat_handle_owner_router();
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
        let mut router = compat_handle_owner_router();
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

    #[test]
    fn core_handle_owner_uses_owner_pending_operation_over_completion_hint() {
        let router = compat_handle_owner_router();
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

        let mut router = McpRouter::new();

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

        let mut router = McpRouter::new();
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

        let mut router = McpRouter::new_with_removal_timeout(Duration::from_secs(60));
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

        let mut router = McpRouter::new_with_removal_timeout(Duration::from_millis(10));
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

        let mut router = McpRouter::new();
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

        let mut router = McpRouter::new();

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
