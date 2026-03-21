// @generated — protocol helpers for `ops_barrier_satisfaction`
// Producer: OpsLifecycleMachine, Effect: WaitAllSatisfied
// Consumer: TurnExecutionMachine, Input: OpsBarrierSatisfied
// Closure policy: AckRequired
//
// This module enforces the cross-machine handoff protocol between OpsLifecycle
// and TurnExecution. When OpsLifecycle emits `WaitAllSatisfied` (barrier ops
// have completed), the owner (agent loop) must submit `OpsBarrierSatisfied`
// to TurnExecution before proceeding to `ToolCallsResolved`. The obligation
// token ensures this feedback is never skipped.

use crate::lifecycle::identifiers::RunId;
use crate::ops::OperationId;
use crate::turn_execution_authority::{
    TurnExecutionAuthority, TurnExecutionInput, TurnExecutionMutator, TurnExecutionTransition,
};

/// Obligation token for the `ops_barrier_satisfaction` protocol.
///
/// Created when the owner observes `WaitAllSatisfied` from OpsLifecycle;
/// consumed when the owner submits `OpsBarrierSatisfied` to TurnExecution.
/// Move semantics enforce that every barrier completion is acknowledged
/// exactly once.
#[derive(Debug)]
pub struct OpsBarrierSatisfactionObligation {
    /// The operation IDs that were awaited (carried from `WaitAllSatisfied`).
    pub operation_ids: Vec<OperationId>,
}

/// Create an obligation token from the data extracted from a
/// `WaitAllSatisfied` effect.
///
/// The caller (shell) is responsible for extracting `operation_ids` from the
/// `OpsLifecycleEffect::WaitAllSatisfied` variant and passing them here.
/// This function lives in core (not runtime) so the obligation can be
/// consumed by [`submit_ops_barrier_satisfied`] without crossing crate
/// boundaries.
pub fn accept_wait_all_satisfied(
    operation_ids: Vec<OperationId>,
) -> OpsBarrierSatisfactionObligation {
    OpsBarrierSatisfactionObligation { operation_ids }
}

/// Submit `OpsBarrierSatisfied` feedback to TurnExecution, consuming the
/// obligation token.
///
/// Called by the agent loop after `wait_all()` returns and the authority has
/// emitted `WaitAllSatisfied`. The `run_id` is the active run from the
/// agent loop context (not carried by the obligation, since OpsLifecycle
/// has no knowledge of TurnExecution's run concept).
pub fn submit_ops_barrier_satisfied(
    authority: &mut TurnExecutionAuthority,
    _obligation: OpsBarrierSatisfactionObligation,
    run_id: RunId,
) -> Result<TurnExecutionTransition, crate::error::AgentError> {
    authority.apply(TurnExecutionInput::OpsBarrierSatisfied { run_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::identifiers::RunId;
    use crate::ops::{AsyncOpRef, OperationId};
    use crate::turn_execution_authority::{
        ContentShape, TurnExecutionAuthority, TurnExecutionInput, TurnExecutionMutator, TurnPhase,
    };
    use uuid::Uuid;

    fn test_run_id() -> RunId {
        RunId(Uuid::from_u128(1))
    }

    /// Reach WaitingForOps: start run → primitive applied → LLM tool calls.
    fn authority_at_waiting_for_ops() -> (TurnExecutionAuthority, RunId) {
        let mut auth = TurnExecutionAuthority::new();
        let rid = test_run_id();
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: rid.clone(),
        })
        .expect("start run");
        auth.apply(TurnExecutionInput::PrimitiveApplied {
            run_id: rid.clone(),
            admitted_content_shape: ContentShape("text".into()),
            vision_enabled: false,
            image_tool_results_enabled: false,
        })
        .expect("primitive applied");
        auth.apply(TurnExecutionInput::LlmReturnedToolCalls {
            run_id: rid.clone(),
            tool_count: 1,
        })
        .expect("llm returned tool calls");
        assert_eq!(auth.phase(), TurnPhase::WaitingForOps);
        (auth, rid)
    }

    #[test]
    fn obligation_created_and_consumed() {
        let (mut auth, rid) = authority_at_waiting_for_ops();

        let op_id = OperationId::new();
        auth.apply(TurnExecutionInput::RegisterPendingOps {
            run_id: rid.clone(),
            op_refs: vec![AsyncOpRef::barrier(op_id.clone())],
            has_barrier_ops: true,
        })
        .expect("register pending ops");

        // Create obligation from WaitAllSatisfied data
        let obligation = accept_wait_all_satisfied(vec![op_id.clone()]);
        assert_eq!(obligation.operation_ids, vec![op_id]);

        // Submit feedback through the protocol helper
        let transition = submit_ops_barrier_satisfied(&mut auth, obligation, rid.clone())
            .expect("barrier satisfied");
        assert_eq!(transition.next_phase, TurnPhase::WaitingForOps);
        assert!(auth.barrier_satisfied());

        // Now ToolCallsResolved can proceed
        auth.apply(TurnExecutionInput::ToolCallsResolved {
            run_id: rid.clone(),
        })
        .expect("tool calls resolved");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);
    }

    #[test]
    fn obligation_carries_operation_ids() {
        let ids = vec![OperationId::new(), OperationId::new()];
        let obligation = accept_wait_all_satisfied(ids.clone());
        assert_eq!(obligation.operation_ids, ids);
    }

    #[test]
    fn submit_without_barrier_ops_fails() {
        let (mut auth, rid) = authority_at_waiting_for_ops();

        // Register ops without barrier
        auth.apply(TurnExecutionInput::RegisterPendingOps {
            run_id: rid.clone(),
            op_refs: vec![AsyncOpRef::detached(OperationId::new())],
            has_barrier_ops: false,
        })
        .expect("register pending ops");

        let obligation = accept_wait_all_satisfied(vec![]);
        // OpsBarrierSatisfied should fail when barrier_satisfied is already
        // trivially true (no barrier ops registered)
        let result = submit_ops_barrier_satisfied(&mut auth, obligation, rid);
        assert!(result.is_err());
    }
}
