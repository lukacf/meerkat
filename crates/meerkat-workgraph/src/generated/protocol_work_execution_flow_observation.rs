// @generated — protocol helpers for `work_execution_flow_observation`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: FlowLaunchAccepted
// Closure policy: AckRequired

use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionFlowObservationObligation {
    pub binding_id: String,
    pub run_id: String,
}

#[macro_export]
macro_rules! work_execution_flow_observation_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowRunning
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowCompleted
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowFailed { .. }
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowCanceled { .. }
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveRunLost { .. }
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionFlowObservationObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::FlowLaunchAccepted { binding_id, run_id } => {
                Some(WorkExecutionFlowObservationObligation {
                    binding_id: binding_id.clone(),
                    run_id: run_id.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

pub fn submit_observe_flow_running(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowObservationObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowRunning)?;
    Ok(transition)
}

pub fn submit_observe_flow_completed(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowObservationObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowCompleted)?;
    Ok(transition)
}

pub fn submit_observe_flow_failed(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowObservationObligation,
    observed_failure_detail: Option<String>,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowFailed {
        detail: observed_failure_detail,
    })?;
    Ok(transition)
}

pub fn submit_observe_flow_canceled(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowObservationObligation,
    observed_cancellation_detail: Option<String>,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowCanceled {
        detail: observed_cancellation_detail,
    })?;
    Ok(transition)
}

pub fn submit_observe_run_lost(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowObservationObligation,
    lost_run_detail: String,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveRunLost {
        detail: lost_run_detail,
    })?;
    Ok(transition)
}
