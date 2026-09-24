// @generated — protocol helpers for `work_execution_launch_failure_evidence_projection`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: LaunchFailureEvidenceProjectionRequested
// Closure policy: AckRequired

use crate::WorkExecutionEvidenceKind;
use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionLaunchFailureEvidenceProjectionObligation {
    pub binding_id: String,
    pub run_id: String,
    pub detail: String,
    pub kind: WorkExecutionEvidenceKind,
}

#[macro_export]
macro_rules! work_execution_launch_failure_evidence_projection_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ConfirmLaunchFailureEvidenceProjected
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionLaunchFailureEvidenceProjectionObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::LaunchFailureEvidenceProjectionRequested {
                binding_id,
                run_id,
                detail,
                kind,
            } => Some(WorkExecutionLaunchFailureEvidenceProjectionObligation {
                binding_id: binding_id.clone(),
                run_id: run_id.clone(),
                detail: detail.clone(),
                kind: *kind,
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_confirm_launch_failure_evidence_projected(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionLaunchFailureEvidenceProjectionObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition =
        authority.apply(WorkExecutionLifecycleInput::ConfirmLaunchFailureEvidenceProjected)?;
    Ok(transition)
}
