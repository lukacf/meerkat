// @generated — protocol helpers for `surface_completion`
// Composition: external_tool_bundle, Producer: external_tool_surface, Effect: ScheduleSurfaceCompletion
// Closure policy: AckRequired
// Liveness: eventual feedback under surface connection liveness

use crate::external_tool_surface_authority::{
    ExternalToolSurfaceAuthority, ExternalToolSurfaceEffect, ExternalToolSurfaceError,
    ExternalToolSurfaceInput, ExternalToolSurfaceMutator, ExternalToolSurfaceTransition,
    SurfaceDeltaOperation, SurfaceId, TurnNumber,
};

#[derive(Debug, Clone)]
pub struct SurfaceCompletionObligation {
    pub surface_id: SurfaceId,
    pub operation: SurfaceDeltaOperation,
    pub applied_at_turn: TurnNumber,
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
                applied_at_turn,
            } => Some(SurfaceCompletionObligation {
                surface_id: surface_id.clone(),
                operation: operation.clone(),
                applied_at_turn: applied_at_turn.clone(),
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_pending_succeeded(
    authority: &mut ExternalToolSurfaceAuthority,
    obligation: SurfaceCompletionObligation,
) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
    let transition = authority.apply(ExternalToolSurfaceInput::PendingSucceeded {
        surface_id: obligation.surface_id,
        applied_at_turn: obligation.applied_at_turn,
    })?;
    Ok(transition)
}

pub fn submit_pending_failed(
    authority: &mut ExternalToolSurfaceAuthority,
    obligation: SurfaceCompletionObligation,
) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
    let transition = authority.apply(ExternalToolSurfaceInput::PendingFailed {
        surface_id: obligation.surface_id,
        applied_at_turn: obligation.applied_at_turn,
    })?;
    Ok(transition)
}
