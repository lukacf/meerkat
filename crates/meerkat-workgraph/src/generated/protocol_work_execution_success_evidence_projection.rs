// @generated — protocol helpers for `work_execution_success_evidence_projection`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: EvidenceProjectionRequested
// Closure policy: AckRequired

use crate::WorkExecutionEvidenceKind;
use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionSuccessEvidenceProjectionObligation {
    pub binding_id: String,
    pub run_id: String,
    pub kind: WorkExecutionEvidenceKind,
}

#[macro_export]
macro_rules! work_execution_success_evidence_projection_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ConfirmEvidenceProjected
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveRunLost { .. }
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionSuccessEvidenceProjectionObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::EvidenceProjectionRequested {
                binding_id,
                run_id,
                kind,
            } => Some(WorkExecutionSuccessEvidenceProjectionObligation {
                binding_id: binding_id.clone(),
                run_id: run_id.clone(),
                kind: *kind,
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_confirm_evidence_projected(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionSuccessEvidenceProjectionObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ConfirmEvidenceProjected)?;
    Ok(transition)
}

pub fn submit_observe_run_lost(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionSuccessEvidenceProjectionObligation,
    lost_run_detail: String,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveRunLost {
        detail: lost_run_detail,
    })?;
    Ok(transition)
}
