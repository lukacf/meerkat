// @generated — protocol helpers for `work_execution_failure_evidence_projection`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: FlowFailureEvidenceProjectionRequested
// Closure policy: AckRequired

use crate::WorkExecutionEvidenceKind;
use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionFailureEvidenceProjectionObligation {
    pub binding_id: String,
    pub run_id: String,
    pub kind: WorkExecutionEvidenceKind,
}

#[macro_export]
macro_rules! work_execution_failure_evidence_projection_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ConfirmFlowFailureEvidenceProjected
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionFailureEvidenceProjectionObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::FlowFailureEvidenceProjectionRequested {
                binding_id,
                run_id,
                kind,
            } => Some(WorkExecutionFailureEvidenceProjectionObligation {
                binding_id: binding_id.clone(),
                run_id: run_id.clone(),
                kind: *kind,
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_confirm_flow_failure_evidence_projected(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFailureEvidenceProjectionObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition =
        authority.apply(WorkExecutionLifecycleInput::ConfirmFlowFailureEvidenceProjected)?;
    Ok(transition)
}
