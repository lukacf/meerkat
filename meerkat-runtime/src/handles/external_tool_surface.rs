//! Runtime impl of [`meerkat_core::handles::ExternalToolSurfaceHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceHandle, SurfaceDiagnosticSnapshot, SurfaceSnapshot,
};
use meerkat_core::tool_scope::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp, ExternalToolSurfaceStagedOp,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`ExternalToolSurfaceHandle`] impl.
#[derive(Debug)]
pub struct RuntimeExternalToolSurfaceHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeExternalToolSurfaceHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl RuntimeExternalToolSurfaceHandle {
    fn snapshot_entry(
        state: &mm_dsl::MeerkatMachineState,
        surface_id: &str,
    ) -> Option<SurfaceSnapshot> {
        let key = surface_id.to_string();
        if !state.known_surfaces.contains(&key)
            && !state.surface_base_state.contains_key(&key)
            && !state.surface_pending_op.contains_key(&key)
            && !state.surface_staged_op.contains_key(&key)
        {
            return None;
        }
        Some(SurfaceSnapshot {
            surface_id: key.clone(),
            base_state: state
                .surface_base_state
                .get(&key)
                .copied()
                .map(ExternalToolSurfaceBaseState::from),
            pending_op: state
                .surface_pending_op
                .get(&key)
                .copied()
                .map(map_pending_op)
                .unwrap_or(ExternalToolSurfacePendingOp::None),
            staged_op: state
                .surface_staged_op
                .get(&key)
                .copied()
                .map(map_staged_op)
                .unwrap_or(ExternalToolSurfaceStagedOp::None),
            staged_intent_sequence: state.surface_staged_intent_sequence.get(&key).copied(),
            pending_task_sequence: state.surface_pending_task_sequence.get(&key).copied(),
            pending_lineage_sequence: state.surface_pending_lineage_sequence.get(&key).copied(),
            inflight_calls: state.surface_inflight_calls.get(&key).copied().unwrap_or(0),
            last_delta_operation: state
                .surface_last_delta_operation
                .get(&key)
                .copied()
                .map(ExternalToolSurfaceDeltaOperation::from),
            last_delta_phase: state
                .surface_last_delta_phase
                .get(&key)
                .copied()
                .map(ExternalToolSurfaceDeltaPhase::from),
            removal_draining_since_ms: state.surface_draining_since_ms.get(&key).copied(),
            removal_timeout_at_ms: state.surface_removal_timeout_at_ms.get(&key).copied(),
            removal_applied_at_turn: state.surface_removal_applied_at_turn.get(&key).copied(),
        })
    }
}

impl ExternalToolSurfaceHandle for RuntimeExternalToolSurfaceHandle {
    fn register(&self, surface_id: String) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceRegister { surface_id },
            "ExternalToolSurfaceHandle::register",
        )
    }

    fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceStageAdd { surface_id, now_ms },
            "ExternalToolSurfaceHandle::stage_add",
        )
    }

    fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceStageRemove { surface_id, now_ms },
            "ExternalToolSurfaceHandle::stage_remove",
        )
    }

    fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceStageReload { surface_id, now_ms },
            "ExternalToolSurfaceHandle::stage_reload",
        )
    }

    fn apply_boundary(
        &self,
        surface_id: String,
        now_ms: u64,
        current_turn: u64,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceApplyBoundary {
                surface_id,
                now_ms,
                current_turn,
            },
            "ExternalToolSurfaceHandle::apply_boundary",
        )
    }

    fn mark_pending_succeeded(
        &self,
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceMarkPendingSucceeded {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            },
            "ExternalToolSurfaceHandle::mark_pending_succeeded",
        )
    }

    fn mark_pending_failed(
        &self,
        surface_id: String,
        reason: String,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceMarkPendingFailed { surface_id, reason },
            "ExternalToolSurfaceHandle::mark_pending_failed",
        )
    }

    fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceCallStarted { surface_id },
            "ExternalToolSurfaceHandle::call_started",
        )
    }

    fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceCallFinished { surface_id },
            "ExternalToolSurfaceHandle::call_finished",
        )
    }

    fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceFinalizeRemovalClean { surface_id },
            "ExternalToolSurfaceHandle::finalize_removal_clean",
        )
    }

    fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceFinalizeRemovalForced { surface_id },
            "ExternalToolSurfaceHandle::finalize_removal_forced",
        )
    }

    fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceSnapshotAligned { epoch },
            "ExternalToolSurfaceHandle::snapshot_aligned",
        )
    }

    fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceShutdown,
            "ExternalToolSurfaceHandle::shutdown_surface",
        )
    }

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot> {
        let state = self.dsl.snapshot_state();
        Self::snapshot_entry(&state, surface_id)
    }

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
        let state = self.dsl.snapshot_state();
        let mut entries: Vec<SurfaceSnapshot> = state
            .known_surfaces
            .iter()
            .filter_map(|surface_id| Self::snapshot_entry(&state, surface_id))
            .collect();
        entries.sort_by(|a, b| a.surface_id.cmp(&b.surface_id));
        SurfaceDiagnosticSnapshot {
            surface_phase: map_surface_phase(state.surface_phase),
            known_surfaces: state.known_surfaces.clone(),
            visible_surfaces: state.visible_surfaces.clone(),
            snapshot_epoch: state.snapshot_epoch,
            snapshot_aligned_epoch: state.snapshot_aligned_epoch,
            has_pending_or_staged: entries.iter().any(|entry| {
                entry.pending_op != ExternalToolSurfacePendingOp::None
                    || entry.staged_op != ExternalToolSurfaceStagedOp::None
            }),
            entries,
        }
    }

    fn visible_surfaces(&self) -> BTreeSet<String> {
        self.dsl.snapshot_state().visible_surfaces
    }

    fn removing_surfaces(&self) -> BTreeSet<String> {
        self.dsl
            .snapshot_state()
            .surface_base_state
            .into_iter()
            .filter_map(|(surface_id, base_state)| {
                if base_state == mm_dsl::ExternalToolSurfaceBaseState::Removing {
                    Some(surface_id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn pending_surfaces(&self) -> BTreeSet<String> {
        self.dsl
            .snapshot_state()
            .surface_pending_op
            .into_iter()
            .filter_map(|(surface_id, pending_op)| {
                if pending_op == mm_dsl::SurfacePendingOp::None {
                    None
                } else {
                    Some(surface_id)
                }
            })
            .collect()
    }

    fn has_pending_or_staged(&self) -> bool {
        let state = self.dsl.snapshot_state();
        state
            .surface_pending_op
            .values()
            .any(|pending_op| *pending_op != mm_dsl::SurfacePendingOp::None)
            || state
                .surface_staged_op
                .values()
                .any(|staged_op| *staged_op != mm_dsl::SurfaceStagedOp::None)
    }

    fn snapshot_epoch(&self) -> u64 {
        self.dsl.snapshot_state().snapshot_epoch
    }

    fn snapshot_aligned_epoch(&self) -> u64 {
        self.dsl.snapshot_state().snapshot_aligned_epoch
    }
}

/// Exhaustive 1-to-1 projection of the DSL's typed surface phase into the
/// cross-crate contract. Compiler enforces completeness.
fn map_surface_phase(phase: mm_dsl::SurfacePhase) -> ExternalToolSurfaceGlobalPhase {
    match phase {
        mm_dsl::SurfacePhase::Operating => ExternalToolSurfaceGlobalPhase::Operating,
        mm_dsl::SurfacePhase::Shutdown => ExternalToolSurfaceGlobalPhase::Shutdown,
    }
}

fn map_pending_op(op: mm_dsl::SurfacePendingOp) -> ExternalToolSurfacePendingOp {
    match op {
        mm_dsl::SurfacePendingOp::None => ExternalToolSurfacePendingOp::None,
        mm_dsl::SurfacePendingOp::Add => ExternalToolSurfacePendingOp::Add,
        mm_dsl::SurfacePendingOp::Reload => ExternalToolSurfacePendingOp::Reload,
    }
}

fn map_staged_op(op: mm_dsl::SurfaceStagedOp) -> ExternalToolSurfaceStagedOp {
    match op {
        mm_dsl::SurfaceStagedOp::None => ExternalToolSurfaceStagedOp::None,
        mm_dsl::SurfaceStagedOp::Add => ExternalToolSurfaceStagedOp::Add,
        mm_dsl::SurfaceStagedOp::Remove => ExternalToolSurfaceStagedOp::Remove,
        mm_dsl::SurfaceStagedOp::Reload => ExternalToolSurfaceStagedOp::Reload,
    }
}
