// @generated — protocol helpers for `work_execution_flow_launch`
// Composition: workgraph_flow_bundle, Producer: work_execution, Effect: FlowLaunchRequested
// Closure policy: AckRequired

use crate::machines::work_execution_lifecycle::{
    WorkExecutionLifecycleEffect, WorkExecutionLifecycleInput,
    WorkExecutionLifecycleMachineAuthority, WorkExecutionLifecycleMachineMutator,
    WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct WorkExecutionFlowLaunchObligation {
    pub binding_id: String,
    pub run_id: String,
}

#[macro_export]
macro_rules! work_execution_flow_launch_feedback_input_patterns {
    () => {
        $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ConfirmFlowStarted
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowRunning
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowCompleted
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowFailed { .. }
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ObserveFlowCanceled { .. }
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::MarkLaunchUncertain { .. }
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::QuarantineLaunch { .. }
        | $crate::machines::work_execution_lifecycle::WorkExecutionLifecycleInput::ResolveLaunchFailed { .. }
    };
}

pub fn extract_obligations(
    transition: &WorkExecutionLifecycleMachineTransition,
) -> Vec<WorkExecutionFlowLaunchObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            WorkExecutionLifecycleEffect::FlowLaunchRequested { binding_id, run_id } => {
                Some(WorkExecutionFlowLaunchObligation {
                    binding_id: binding_id.clone(),
                    run_id: run_id.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

pub fn submit_confirm_flow_started(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ConfirmFlowStarted)?;
    Ok(transition)
}

pub fn submit_observe_flow_running(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowRunning)?;
    Ok(transition)
}

pub fn submit_observe_flow_completed(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowCompleted)?;
    Ok(transition)
}

pub fn submit_observe_flow_failed(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
    observed_failure_detail: Option<String>,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowFailed {
        detail: observed_failure_detail,
    })?;
    Ok(transition)
}

pub fn submit_observe_flow_canceled(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
    observed_cancellation_detail: Option<String>,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ObserveFlowCanceled {
        detail: observed_cancellation_detail,
    })?;
    Ok(transition)
}

pub fn submit_mark_launch_uncertain(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
    launch_uncertainty_detail: String,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::MarkLaunchUncertain {
        detail: launch_uncertainty_detail,
    })?;
    Ok(transition)
}

pub fn submit_quarantine_launch(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
    launch_quarantine_detail: String,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::QuarantineLaunch {
        detail: launch_quarantine_detail,
    })?;
    Ok(transition)
}

pub fn submit_resolve_launch_failed(
    authority: &mut WorkExecutionLifecycleMachineAuthority,
    _obligation: WorkExecutionFlowLaunchObligation,
    launch_failure_detail: String,
) -> Result<WorkExecutionLifecycleMachineTransition, WorkExecutionLifecycleMachineTransitionError> {
    let transition = authority.apply(WorkExecutionLifecycleInput::ResolveLaunchFailed {
        detail: launch_failure_detail,
    })?;
    Ok(transition)
}
