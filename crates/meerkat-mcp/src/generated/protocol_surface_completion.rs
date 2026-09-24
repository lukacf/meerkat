// @generated — protocol helpers for `surface_completion`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: ScheduleSurfaceCompletion
// Closure policy: AckRequired
// Liveness: eventual feedback under surface connection liveness

use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceEffect, ExternalToolSurfaceHandle,
};
use meerkat_core::tool_scope::{
    ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceFailureCause,
};

#[derive(Debug, Clone)]
pub struct SurfaceCompletionObligation {
    pub surface_id: String,
    pub operation: ExternalToolSurfaceDeltaOperation,
    pub pending_task_sequence: u64,
    pub staged_intent_sequence: u64,
    pub applied_at_turn: u64,
}

pub fn extract_obligations(
    effects: &[ExternalToolSurfaceEffect],
) -> Vec<SurfaceCompletionObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => Some(SurfaceCompletionObligation {
                surface_id: surface_id.clone(),
                operation: *operation,
                pending_task_sequence: *pending_task_sequence,
                staged_intent_sequence: *staged_intent_sequence,
                applied_at_turn: *applied_at_turn,
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_surface_mark_pending_succeeded(
    handle: &(impl ExternalToolSurfaceHandle + ?Sized),
    obligation: SurfaceCompletionObligation,
) -> Result<(), DslTransitionError> {
    handle.mark_pending_succeeded(
        obligation.surface_id,
        obligation.pending_task_sequence,
        obligation.staged_intent_sequence,
    )
}

pub fn submit_surface_mark_pending_failed(
    handle: &(impl ExternalToolSurfaceHandle + ?Sized),
    obligation: SurfaceCompletionObligation,
    cause: ExternalToolSurfaceFailureCause,
) -> Result<(), DslTransitionError> {
    handle.mark_pending_failed(
        obligation.surface_id,
        obligation.pending_task_sequence,
        obligation.staged_intent_sequence,
        cause,
    )
}
