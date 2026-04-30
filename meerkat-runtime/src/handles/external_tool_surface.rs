//! Runtime impl of [`meerkat_core::handles::ExternalToolSurfaceHandle`].

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceEffect, ExternalToolSurfaceHandle,
    ExternalToolSurfaceInput, ExternalToolSurfaceTransition, SurfaceDiagnosticSnapshot,
    SurfaceSnapshot,
};
use meerkat_core::tool_scope::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceFailureCause, ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp,
    ExternalToolSurfaceStagedOp,
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
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Attached,
            ..Default::default()
        };
        let shared = Arc::new(std::sync::Mutex::new(
            mm_dsl::MeerkatMachineAuthority::from_state(state),
        ));
        Self::new(Arc::new(HandleDslAuthority::from_shared(shared)))
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

    fn apply_input_with_effects(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
        let effects = self.dsl.apply_input_with_effects(input, context)?;
        let state = self.dsl.snapshot_state();
        Ok(ExternalToolSurfaceTransition {
            phase: map_surface_phase(state.surface_phase),
            effects: effects
                .into_iter()
                .filter_map(|effect| map_surface_effect(effect, state.snapshot_epoch))
                .collect(),
        })
    }
}

impl ExternalToolSurfaceHandle for RuntimeExternalToolSurfaceHandle {
    fn apply_surface_input(
        &self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, DslTransitionError> {
        match input {
            ExternalToolSurfaceInput::StageAdd { surface_id, now_ms } => self
                .apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::SurfaceStageAdd { surface_id, now_ms },
                    "ExternalToolSurfaceHandle::stage_add",
                ),
            ExternalToolSurfaceInput::StageRemove { surface_id, now_ms } => self
                .apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::SurfaceStageRemove { surface_id, now_ms },
                    "ExternalToolSurfaceHandle::stage_remove",
                ),
            ExternalToolSurfaceInput::StageReload { surface_id, now_ms } => self
                .apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::SurfaceStageReload { surface_id, now_ms },
                    "ExternalToolSurfaceHandle::stage_reload",
                ),
            ExternalToolSurfaceInput::ApplyBoundary {
                surface_id,
                now_ms,
                staged_intent_sequence,
                applied_at_turn,
            } => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceApplyBoundary {
                    surface_id,
                    now_ms,
                    staged_intent_sequence,
                    applied_at_turn,
                },
                "ExternalToolSurfaceHandle::apply_boundary",
            ),
            ExternalToolSurfaceInput::MarkPendingSucceeded {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
            } => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceMarkPendingSucceeded {
                    surface_id,
                    pending_task_sequence,
                    staged_intent_sequence,
                },
                "ExternalToolSurfaceHandle::mark_pending_succeeded",
            ),
            ExternalToolSurfaceInput::MarkPendingFailed {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause,
            } => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceMarkPendingFailed {
                    surface_id,
                    pending_task_sequence,
                    staged_intent_sequence,
                    cause: mm_dsl::ExternalToolSurfaceFailureCause::from(cause),
                },
                "ExternalToolSurfaceHandle::mark_pending_failed",
            ),
            ExternalToolSurfaceInput::CallStarted { surface_id } => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceCallStarted { surface_id },
                "ExternalToolSurfaceHandle::call_started",
            ),
            ExternalToolSurfaceInput::CallFinished { surface_id } => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceCallFinished { surface_id },
                "ExternalToolSurfaceHandle::call_finished",
            ),
            ExternalToolSurfaceInput::FinalizeRemovalClean { surface_id } => self
                .apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::SurfaceFinalizeRemovalClean { surface_id },
                    "ExternalToolSurfaceHandle::finalize_removal_clean",
                ),
            ExternalToolSurfaceInput::FinalizeRemovalForced { surface_id } => self
                .apply_input_with_effects(
                    mm_dsl::MeerkatMachineInput::SurfaceFinalizeRemovalForced { surface_id },
                    "ExternalToolSurfaceHandle::finalize_removal_forced",
                ),
            ExternalToolSurfaceInput::SnapshotAligned { epoch } => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceSnapshotAligned { epoch },
                "ExternalToolSurfaceHandle::snapshot_aligned",
            ),
            ExternalToolSurfaceInput::Shutdown => self.apply_input_with_effects(
                mm_dsl::MeerkatMachineInput::SurfaceShutdown,
                "ExternalToolSurfaceHandle::shutdown_surface",
            ),
        }
    }

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
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceApplyBoundary {
                surface_id,
                now_ms,
                staged_intent_sequence,
                applied_at_turn,
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
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        cause: ExternalToolSurfaceFailureCause,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SurfaceMarkPendingFailed {
                surface_id,
                pending_task_sequence,
                staged_intent_sequence,
                cause: mm_dsl::ExternalToolSurfaceFailureCause::from(cause),
            },
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

fn map_surface_effect(
    effect: mm_dsl::MeerkatMachineEffect,
    _snapshot_epoch: u64,
) -> Option<ExternalToolSurfaceEffect> {
    match effect {
        mm_dsl::MeerkatMachineEffect::ScheduleSurfaceCompletion {
            surface_id,
            operation,
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn,
        } => Some(ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
            surface_id,
            operation: ExternalToolSurfaceDeltaOperation::from(operation),
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn,
        }),
        mm_dsl::MeerkatMachineEffect::RefreshVisibleSurfaceSet { snapshot_epoch } => {
            Some(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch })
        }
        mm_dsl::MeerkatMachineEffect::EmitExternalToolDelta {
            surface_id,
            operation,
            phase,
            cause,
        } => Some(ExternalToolSurfaceEffect::EmitExternalToolDelta {
            surface_id,
            operation: ExternalToolSurfaceDeltaOperation::from(operation),
            phase: ExternalToolSurfaceDeltaPhase::from(phase),
            cause: cause.map(ExternalToolSurfaceFailureCause::from),
        }),
        mm_dsl::MeerkatMachineEffect::CloseSurfaceConnection { surface_id } => {
            Some(ExternalToolSurfaceEffect::CloseSurfaceConnection { surface_id })
        }
        mm_dsl::MeerkatMachineEffect::RejectSurfaceCall { surface_id, cause } => {
            Some(ExternalToolSurfaceEffect::RejectSurfaceCall {
                surface_id,
                cause: ExternalToolSurfaceFailureCause::from(cause),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use meerkat_core::ExternalToolSurfaceFailureCause;

    fn handle_in_phase(phase: mm_dsl::MeerkatPhase) -> RuntimeExternalToolSurfaceHandle {
        let state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: phase,
            ..Default::default()
        };
        let authority = mm_dsl::MeerkatMachineAuthority::from_state(state);
        let shared = Arc::new(Mutex::new(authority));
        RuntimeExternalToolSurfaceHandle::new(Arc::new(HandleDslAuthority::from_shared(shared)))
    }

    fn handle_with_active_surface(surface_id: &str) -> RuntimeExternalToolSurfaceHandle {
        let mut state = mm_dsl::MeerkatMachineState {
            lifecycle_phase: mm_dsl::MeerkatPhase::Attached,
            ..Default::default()
        };
        state.known_surfaces.insert(surface_id.to_owned());
        state.active_surfaces.insert(surface_id.to_owned());
        state.surface_base_state.insert(
            surface_id.to_owned(),
            mm_dsl::ExternalToolSurfaceBaseState::Active,
        );
        let authority = mm_dsl::MeerkatMachineAuthority::from_state(state);
        let shared = Arc::new(Mutex::new(authority));
        RuntimeExternalToolSurfaceHandle::new(Arc::new(HandleDslAuthority::from_shared(shared)))
    }

    #[test]
    fn staging_inputs_mint_sequences_and_project_snapshot() {
        let handle = handle_in_phase(mm_dsl::MeerkatPhase::Attached);

        handle.stage_add("alpha".to_owned(), 10).expect("stage add");
        let add = handle.surface_snapshot("alpha").expect("add snapshot");
        assert_eq!(add.staged_op, ExternalToolSurfaceStagedOp::Add);
        assert_eq!(add.staged_intent_sequence, Some(1));
        assert!(
            handle
                .diagnostic_snapshot()
                .known_surfaces
                .contains("alpha")
        );

        handle
            .stage_remove("alpha".to_owned(), 20)
            .expect("stage remove");
        let remove = handle.surface_snapshot("alpha").expect("remove snapshot");
        assert_eq!(remove.staged_op, ExternalToolSurfaceStagedOp::Remove);
        assert_eq!(remove.staged_intent_sequence, Some(2));

        handle.stage_add("beta".to_owned(), 30).expect("stage add");
        let beta = handle.surface_snapshot("beta").expect("beta snapshot");
        assert_eq!(beta.staged_op, ExternalToolSurfaceStagedOp::Add);
        assert_eq!(beta.staged_intent_sequence, Some(3));
    }

    #[test]
    fn staging_inputs_reject_after_surface_shutdown() {
        let handle = handle_in_phase(mm_dsl::MeerkatPhase::Attached);

        handle.shutdown_surface().expect("shutdown surface");

        assert!(handle.stage_add("alpha".to_owned(), 10).is_err());
        assert_eq!(
            handle.diagnostic_snapshot().surface_phase,
            ExternalToolSurfaceGlobalPhase::Shutdown
        );
    }

    #[test]
    fn stage_reload_requires_active_base_state() {
        let handle = handle_in_phase(mm_dsl::MeerkatPhase::Attached);

        assert!(handle.stage_reload("alpha".to_owned(), 10).is_err());
        assert!(handle.surface_snapshot("alpha").is_none());
    }

    #[test]
    fn stage_reload_mints_sequence_for_active_surface() {
        let handle = handle_with_active_surface("alpha");

        handle
            .stage_reload("alpha".to_owned(), 10)
            .expect("stage reload");

        let snapshot = handle.surface_snapshot("alpha").expect("reload snapshot");
        assert_eq!(
            snapshot.base_state,
            Some(ExternalToolSurfaceBaseState::Active)
        );
        assert_eq!(snapshot.staged_op, ExternalToolSurfaceStagedOp::Reload);
        assert_eq!(snapshot.staged_intent_sequence, Some(1));
    }

    #[test]
    fn runtime_surface_lifecycle_keeps_pending_lineage_on_staged_sequence() {
        let handle = handle_in_phase(mm_dsl::MeerkatPhase::Attached);

        assert!(handle.stage_reload("alpha".to_owned(), 10).is_err());

        handle.stage_add("alpha".to_owned(), 10).expect("stage add");
        let staged_add = handle.surface_snapshot("alpha").expect("staged add");
        let add_lineage = staged_add
            .staged_intent_sequence
            .expect("staged add sequence");

        assert!(
            handle
                .apply_boundary("alpha".to_owned(), 20, add_lineage + 1, 99)
                .is_err(),
            "apply boundary must reject a lineage that is not the staged intent"
        );

        handle
            .apply_boundary("alpha".to_owned(), 20, add_lineage, 99)
            .expect("apply add boundary");
        let pending_add = handle.surface_snapshot("alpha").expect("pending add");
        assert_eq!(pending_add.pending_op, ExternalToolSurfacePendingOp::Add);
        assert_eq!(pending_add.pending_task_sequence, Some(1));
        assert_eq!(pending_add.pending_lineage_sequence, Some(add_lineage));
        assert_eq!(pending_add.staged_op, ExternalToolSurfaceStagedOp::None);

        handle
            .mark_pending_succeeded("alpha".to_owned(), 1, add_lineage)
            .expect("add success");
        let active = handle.surface_snapshot("alpha").expect("active alpha");
        assert_eq!(
            active.base_state,
            Some(ExternalToolSurfaceBaseState::Active)
        );
        assert!(handle.visible_surfaces().contains("alpha"));

        handle
            .stage_reload("alpha".to_owned(), 30)
            .expect("stage reload after active");
        let staged_reload = handle.surface_snapshot("alpha").expect("staged reload");
        let reload_lineage = staged_reload
            .staged_intent_sequence
            .expect("staged reload sequence");
        handle
            .apply_boundary("alpha".to_owned(), 40, reload_lineage, 100)
            .expect("apply reload boundary");
        let pending_reload = handle.surface_snapshot("alpha").expect("pending reload");
        assert_eq!(
            pending_reload.pending_op,
            ExternalToolSurfacePendingOp::Reload
        );
        assert_eq!(
            pending_reload.pending_lineage_sequence,
            Some(reload_lineage)
        );
        handle
            .mark_pending_succeeded("alpha".to_owned(), 2, reload_lineage)
            .expect("reload success");

        handle
            .stage_remove("alpha".to_owned(), 50)
            .expect("stage remove");
        let remove_lineage = handle
            .surface_snapshot("alpha")
            .and_then(|entry| entry.staged_intent_sequence)
            .expect("staged remove sequence");
        handle
            .apply_boundary("alpha".to_owned(), 60, remove_lineage, 101)
            .expect("apply remove boundary");
        let removing = handle.surface_snapshot("alpha").expect("removing alpha");
        assert_eq!(
            removing.base_state,
            Some(ExternalToolSurfaceBaseState::Removing)
        );
        assert!(!handle.visible_surfaces().contains("alpha"));

        handle
            .finalize_removal_clean("alpha".to_owned())
            .expect("finalize removal");
        let removed = handle.surface_snapshot("alpha").expect("removed alpha");
        assert_eq!(
            removed.base_state,
            Some(ExternalToolSurfaceBaseState::Removed)
        );
        assert!(!handle.visible_surfaces().contains("alpha"));
    }

    #[test]
    fn runtime_mark_pending_failed_accepts_typed_failure_cause() {
        let handle = handle_in_phase(mm_dsl::MeerkatPhase::Attached);

        handle.stage_add("alpha".to_owned(), 10).expect("stage add");
        let staged_sequence = handle
            .surface_snapshot("alpha")
            .and_then(|entry| entry.staged_intent_sequence)
            .expect("staged add sequence");
        handle
            .apply_boundary("alpha".to_owned(), 20, staged_sequence, 99)
            .expect("apply add boundary");

        let transition = handle
            .apply_surface_input(ExternalToolSurfaceInput::MarkPendingFailed {
                surface_id: "alpha".to_owned(),
                pending_task_sequence: 1,
                staged_intent_sequence: staged_sequence,
                cause: ExternalToolSurfaceFailureCause::PendingFailed,
            })
            .expect("typed pending failure");
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            ExternalToolSurfaceEffect::EmitExternalToolDelta {
                phase: ExternalToolSurfaceDeltaPhase::Failed,
                cause: Some(ExternalToolSurfaceFailureCause::PendingFailed),
                ..
            }
        )));

        let failed = handle.surface_snapshot("alpha").expect("failed alpha");
        assert_eq!(
            failed.last_delta_phase,
            Some(ExternalToolSurfaceDeltaPhase::Failed)
        );
        assert_eq!(failed.pending_op, ExternalToolSurfacePendingOp::None);
    }
}
