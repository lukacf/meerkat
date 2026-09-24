// @generated — protocol helpers for `work_execution_work_closure`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: WorkClosureRequested
// Closure policy: AckRequired

use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionWorkClosureObligation {
    pub binding_id: String,
}

#[macro_export]
macro_rules! work_execution_work_closure_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ConfirmWorkClosed
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::RefuseWorkClosure { .. }
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionWorkClosureObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::WorkClosureRequested { binding_id } => {
                Some(WorkExecutionWorkClosureObligation {
                    binding_id: binding_id.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

pub fn submit_confirm_work_closed(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionWorkClosureObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ConfirmWorkClosed)?;
    Ok(transition)
}

pub fn submit_refuse_work_closure(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionWorkClosureObligation,
    work_closure_refusal_detail: String,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::RefuseWorkClosure {
        detail: work_closure_refusal_detail,
    })?;
    Ok(transition)
}
