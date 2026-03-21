// @generated — protocol helpers for `surface_completion`
// Producer: ExternalToolSurfaceMachine, Effect: ScheduleSurfaceCompletion
// Closure policy: AckRequired
//
// This module enforces the surface completion handoff protocol. When a staged
// surface operation (add/reload) is applied at a turn boundary, the authority
// emits `ScheduleSurfaceCompletion`. The MCP host must spawn a connection task
// and eventually feed back `PendingSucceeded` or `PendingFailed`.

use crate::external_tool_surface_authority::{
    ExternalToolSurfaceAuthority, ExternalToolSurfaceEffect, ExternalToolSurfaceError,
    ExternalToolSurfaceInput, ExternalToolSurfaceMutator, ExternalToolSurfaceTransition,
    SurfaceDeltaOperation, SurfaceId, TurnNumber,
};

/// Obligation token for the `surface_completion` protocol.
///
/// Created when `ScheduleSurfaceCompletion` is emitted; consumed when the
/// owner submits `PendingSucceeded` or `PendingFailed` feedback. Move
/// semantics enforce that every scheduled completion is acknowledged exactly
/// once.
#[derive(Debug)]
pub struct SurfaceCompletionObligation {
    /// The surface being connected/reconnected.
    pub surface_id: SurfaceId,
    /// The operation that triggered the completion (Add or Reload).
    pub operation: SurfaceDeltaOperation,
}

/// Extract obligation tokens from a transition's effects.
///
/// Returns the original effects list plus any obligations extracted from
/// `ScheduleSurfaceCompletion` effects.
pub fn extract_obligations(
    effects: &[ExternalToolSurfaceEffect],
) -> Vec<SurfaceCompletionObligation> {
    effects
        .iter()
        .filter_map(|e| match e {
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id,
                operation,
            } => Some(SurfaceCompletionObligation {
                surface_id: surface_id.clone(),
                operation: *operation,
            }),
            _ => None,
        })
        .collect()
}

/// Submit `PendingSucceeded` feedback, consuming the obligation token.
///
/// Called when the MCP connection task has successfully connected and
/// enumerated tools for the surface.
pub fn submit_pending_succeeded(
    authority: &mut ExternalToolSurfaceAuthority,
    obligation: SurfaceCompletionObligation,
    applied_at_turn: TurnNumber,
) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
    authority.apply(ExternalToolSurfaceInput::PendingSucceeded {
        surface_id: obligation.surface_id,
        applied_at_turn,
    })
}

/// Submit `PendingFailed` feedback, consuming the obligation token.
///
/// Called when the MCP connection task failed (timeout, auth error, etc.).
pub fn submit_pending_failed(
    authority: &mut ExternalToolSurfaceAuthority,
    obligation: SurfaceCompletionObligation,
    applied_at_turn: TurnNumber,
) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
    authority.apply(ExternalToolSurfaceInput::PendingFailed {
        surface_id: obligation.surface_id,
        applied_at_turn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_tool_surface_authority::{
        ExternalToolSurfaceAuthority, ExternalToolSurfaceEffect, ExternalToolSurfaceInput,
        ExternalToolSurfaceMutator, SurfaceDeltaOperation, SurfaceId, TurnNumber,
    };

    fn make_authority() -> ExternalToolSurfaceAuthority {
        ExternalToolSurfaceAuthority::new()
    }

    fn sid(name: &str) -> SurfaceId {
        SurfaceId::from(name)
    }

    fn turn(n: u64) -> TurnNumber {
        TurnNumber(n)
    }

    /// Reach a state where ScheduleSurfaceCompletion has been emitted.
    fn authority_with_pending_add(
        name: &str,
    ) -> (ExternalToolSurfaceAuthority, SurfaceCompletionObligation) {
        let mut auth = make_authority();
        // Stage an add
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid(name),
        })
        .expect("stage add");
        // Apply at boundary
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid(name),
                applied_at_turn: turn(1),
            })
            .expect("apply boundary");
        let obligations = extract_obligations(&t.effects);
        assert_eq!(obligations.len(), 1);
        let obligation = obligations.into_iter().next().expect("one obligation");
        assert_eq!(obligation.surface_id, sid(name));
        assert_eq!(obligation.operation, SurfaceDeltaOperation::Add);
        (auth, obligation)
    }

    #[test]
    fn add_succeeded_through_protocol() {
        let (mut auth, obligation) = authority_with_pending_add("server_a");
        let t =
            submit_pending_succeeded(&mut auth, obligation, turn(1)).expect("pending succeeded");
        assert_eq!(t.transition_name, "PendingSucceededAdd");
        assert!(auth.is_visible(&sid("server_a")));
    }

    #[test]
    fn add_failed_through_protocol() {
        let (mut auth, obligation) = authority_with_pending_add("server_b");
        let t = submit_pending_failed(&mut auth, obligation, turn(1)).expect("pending failed");
        assert_eq!(t.transition_name, "PendingFailedAdd");
        assert!(!auth.is_visible(&sid("server_b")));
    }

    #[test]
    fn extract_obligations_filters_non_schedule_effects() {
        let effects = vec![
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet,
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                surface_id: sid("test"),
                operation: SurfaceDeltaOperation::Add,
            },
        ];
        let obligations = extract_obligations(&effects);
        assert_eq!(obligations.len(), 1);
        assert_eq!(obligations[0].surface_id, sid("test"));
    }

    #[test]
    fn reload_succeeded_through_protocol() {
        let mut auth = make_authority();
        // First: add and succeed
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid("srv"),
        })
        .expect("stage add");
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid("srv"),
                applied_at_turn: turn(1),
            })
            .expect("apply boundary add");
        let oblig = extract_obligations(&t.effects)
            .into_iter()
            .next()
            .expect("add obligation");
        submit_pending_succeeded(&mut auth, oblig, turn(1)).expect("add succeeded");

        // Now: stage reload
        auth.apply(ExternalToolSurfaceInput::StageReload {
            surface_id: sid("srv"),
        })
        .expect("stage reload");
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid("srv"),
                applied_at_turn: turn(2),
            })
            .expect("apply boundary reload");
        let obligations = extract_obligations(&t.effects);
        assert_eq!(obligations.len(), 1);
        assert_eq!(obligations[0].operation, SurfaceDeltaOperation::Reload);

        let oblig = obligations.into_iter().next().expect("reload obligation");
        let t = submit_pending_succeeded(&mut auth, oblig, turn(2)).expect("reload succeeded");
        assert_eq!(t.transition_name, "PendingSucceededReload");
    }
}
