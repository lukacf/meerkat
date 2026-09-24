// @generated — protocol helpers for `work_execution_cancellation_evidence_projection`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: FlowCancellationEvidenceProjectionRequested
// Closure policy: AckRequired

use crate::WorkExecutionEvidenceKind;
use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionCancellationEvidenceProjectionObligation {
    pub binding_id: String,
    pub run_id: String,
    pub kind: WorkExecutionEvidenceKind,
}

#[macro_export]
macro_rules! work_execution_cancellation_evidence_projection_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ConfirmFlowCancellationEvidenceProjected
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionCancellationEvidenceProjectionObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::FlowCancellationEvidenceProjectionRequested {
                binding_id,
                run_id,
                kind,
            } => Some(WorkExecutionCancellationEvidenceProjectionObligation {
                binding_id: binding_id.clone(),
                run_id: run_id.clone(),
                kind: *kind,
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_confirm_flow_cancellation_evidence_projected(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionCancellationEvidenceProjectionObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition =
        authority.apply(WorkExecutionLifecycleInput::ConfirmFlowCancellationEvidenceProjected)?;
    Ok(transition)
}
