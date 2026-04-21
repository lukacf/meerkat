// Local flow-run kernel wrapper derived from the former compat codegen.
#![allow(dead_code)]
use meerkat_machine_kernels::{
    GeneratedMachineKernel, KernelEffectVariant, KernelField, KernelFields, KernelHelperName,
    KernelInput, KernelInputVariant, KernelPhase, KernelSignal, KernelSignalVariant, KernelState,
    KernelTransitionName, KernelValue, TransitionOutcome, TransitionRefusal,
};

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    super::flow_run_machine_schema::flow_run_machine()
}

pub fn kernel() -> GeneratedMachineKernel {
    GeneratedMachineKernel::new(schema())
}

pub fn fields(entries: impl IntoIterator<Item = (KernelField, KernelValue)>) -> KernelFields {
    entries.into_iter().collect()
}

pub fn input(variant: KernelInputVariant, fields: KernelFields) -> KernelInput {
    KernelInput::new(variant, fields)
}

pub fn signal(variant: KernelSignalVariant, fields: KernelFields) -> KernelSignal {
    KernelSignal::new(variant, fields)
}

pub mod phase {
    #[must_use]
    pub fn absent() -> super::KernelPhase {
        super::KernelPhase::new_static("Absent")
    }
    #[must_use]
    pub fn pending() -> super::KernelPhase {
        super::KernelPhase::new_static("Pending")
    }
    #[must_use]
    pub fn running() -> super::KernelPhase {
        super::KernelPhase::new_static("Running")
    }
    #[must_use]
    pub fn completed() -> super::KernelPhase {
        super::KernelPhase::new_static("Completed")
    }
    #[must_use]
    pub fn failed() -> super::KernelPhase {
        super::KernelPhase::new_static("Failed")
    }
    #[must_use]
    pub fn canceled() -> super::KernelPhase {
        super::KernelPhase::new_static("Canceled")
    }
}

pub mod field {
    #[must_use]
    pub fn tracked_steps() -> super::KernelField {
        super::KernelField::new_static("tracked_steps")
    }
    #[must_use]
    pub fn ordered_steps() -> super::KernelField {
        super::KernelField::new_static("ordered_steps")
    }
    #[must_use]
    pub fn step_status() -> super::KernelField {
        super::KernelField::new_static("step_status")
    }
    #[must_use]
    pub fn output_recorded() -> super::KernelField {
        super::KernelField::new_static("output_recorded")
    }
    #[must_use]
    pub fn step_condition_results() -> super::KernelField {
        super::KernelField::new_static("step_condition_results")
    }
    #[must_use]
    pub fn step_has_conditions() -> super::KernelField {
        super::KernelField::new_static("step_has_conditions")
    }
    #[must_use]
    pub fn step_dependencies() -> super::KernelField {
        super::KernelField::new_static("step_dependencies")
    }
    #[must_use]
    pub fn step_dependency_modes() -> super::KernelField {
        super::KernelField::new_static("step_dependency_modes")
    }
    #[must_use]
    pub fn step_branches() -> super::KernelField {
        super::KernelField::new_static("step_branches")
    }
    #[must_use]
    pub fn step_collection_policies() -> super::KernelField {
        super::KernelField::new_static("step_collection_policies")
    }
    #[must_use]
    pub fn step_quorum_thresholds() -> super::KernelField {
        super::KernelField::new_static("step_quorum_thresholds")
    }
    #[must_use]
    pub fn step_target_counts() -> super::KernelField {
        super::KernelField::new_static("step_target_counts")
    }
    #[must_use]
    pub fn step_target_success_counts() -> super::KernelField {
        super::KernelField::new_static("step_target_success_counts")
    }
    #[must_use]
    pub fn step_target_terminal_failure_counts() -> super::KernelField {
        super::KernelField::new_static("step_target_terminal_failure_counts")
    }
    #[must_use]
    pub fn target_retry_counts() -> super::KernelField {
        super::KernelField::new_static("target_retry_counts")
    }
    #[must_use]
    pub fn failure_count() -> super::KernelField {
        super::KernelField::new_static("failure_count")
    }
    #[must_use]
    pub fn consecutive_failure_count() -> super::KernelField {
        super::KernelField::new_static("consecutive_failure_count")
    }
    #[must_use]
    pub fn escalation_threshold() -> super::KernelField {
        super::KernelField::new_static("escalation_threshold")
    }
    #[must_use]
    pub fn max_step_retries() -> super::KernelField {
        super::KernelField::new_static("max_step_retries")
    }
    #[must_use]
    pub fn ready_frames() -> super::KernelField {
        super::KernelField::new_static("ready_frames")
    }
    #[must_use]
    pub fn ready_frame_membership() -> super::KernelField {
        super::KernelField::new_static("ready_frame_membership")
    }
    #[must_use]
    pub fn pending_body_frame_loops() -> super::KernelField {
        super::KernelField::new_static("pending_body_frame_loops")
    }
    #[must_use]
    pub fn pending_body_frame_loop_membership() -> super::KernelField {
        super::KernelField::new_static("pending_body_frame_loop_membership")
    }
    #[must_use]
    pub fn active_node_count() -> super::KernelField {
        super::KernelField::new_static("active_node_count")
    }
    #[must_use]
    pub fn active_frame_count() -> super::KernelField {
        super::KernelField::new_static("active_frame_count")
    }
    #[must_use]
    pub fn max_active_nodes() -> super::KernelField {
        super::KernelField::new_static("max_active_nodes")
    }
    #[must_use]
    pub fn max_active_frames() -> super::KernelField {
        super::KernelField::new_static("max_active_frames")
    }
    #[must_use]
    pub fn max_frame_depth() -> super::KernelField {
        super::KernelField::new_static("max_frame_depth")
    }
    #[must_use]
    pub fn last_granted_frame() -> super::KernelField {
        super::KernelField::new_static("last_granted_frame")
    }
    #[must_use]
    pub fn last_granted_loop() -> super::KernelField {
        super::KernelField::new_static("last_granted_loop")
    }
    #[must_use]
    pub fn step_ids() -> super::KernelField {
        super::KernelField::new_static("step_ids")
    }
    #[must_use]
    pub fn step_id() -> super::KernelField {
        super::KernelField::new_static("step_id")
    }
    #[must_use]
    pub fn append_failure_ledger() -> super::KernelField {
        super::KernelField::new_static("append_failure_ledger")
    }
    #[must_use]
    pub fn target_count() -> super::KernelField {
        super::KernelField::new_static("target_count")
    }
    #[must_use]
    pub fn target_id() -> super::KernelField {
        super::KernelField::new_static("target_id")
    }
    #[must_use]
    pub fn retry_key() -> super::KernelField {
        super::KernelField::new_static("retry_key")
    }
    #[must_use]
    pub fn frame_id() -> super::KernelField {
        super::KernelField::new_static("frame_id")
    }
    #[must_use]
    pub fn loop_instance_id() -> super::KernelField {
        super::KernelField::new_static("loop_instance_id")
    }
    #[must_use]
    pub fn depth() -> super::KernelField {
        super::KernelField::new_static("depth")
    }
    #[must_use]
    pub fn run_status() -> super::KernelField {
        super::KernelField::new_static("run_status")
    }
    #[must_use]
    pub fn expected_status() -> super::KernelField {
        super::KernelField::new_static("expected_status")
    }
    #[must_use]
    pub fn expected() -> super::KernelField {
        super::KernelField::new_static("expected")
    }
    #[must_use]
    pub fn allowed_statuses() -> super::KernelField {
        super::KernelField::new_static("allowed_statuses")
    }
    #[must_use]
    pub fn status() -> super::KernelField {
        super::KernelField::new_static("status")
    }
}

pub mod input {
    #[must_use]
    pub fn create_run() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("CreateRun")
    }
    #[must_use]
    pub fn start_run() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("StartRun")
    }
    #[must_use]
    pub fn dispatch_step() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("DispatchStep")
    }
    #[must_use]
    pub fn complete_step() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("CompleteStep")
    }
    #[must_use]
    pub fn record_step_output() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RecordStepOutput")
    }
    #[must_use]
    pub fn condition_passed() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("ConditionPassed")
    }
    #[must_use]
    pub fn condition_rejected() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("ConditionRejected")
    }
    #[must_use]
    pub fn fail_step() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("FailStep")
    }
    #[must_use]
    pub fn skip_step() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("SkipStep")
    }
    #[must_use]
    pub fn project_frame_step_status() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("ProjectFrameStepStatus")
    }
    #[must_use]
    pub fn cancel_step() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("CancelStep")
    }
    #[must_use]
    pub fn register_targets() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RegisterTargets")
    }
    #[must_use]
    pub fn record_target_success() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RecordTargetSuccess")
    }
    #[must_use]
    pub fn record_target_terminal_failure() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RecordTargetTerminalFailure")
    }
    #[must_use]
    pub fn record_target_canceled() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RecordTargetCanceled")
    }
    #[must_use]
    pub fn record_target_failure() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RecordTargetFailure")
    }
    #[must_use]
    pub fn register_ready_frame() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RegisterReadyFrame")
    }
    #[must_use]
    pub fn pump_node_scheduler() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("PumpNodeScheduler")
    }
    #[must_use]
    pub fn register_pending_body_frame() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RegisterPendingBodyFrame")
    }
    #[must_use]
    pub fn pump_frame_scheduler() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("PumpFrameScheduler")
    }
    #[must_use]
    pub fn node_execution_released() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("NodeExecutionReleased")
    }
    #[must_use]
    pub fn frame_terminated() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("FrameTerminated")
    }
    #[must_use]
    pub fn terminalize_completed() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("TerminalizeCompleted")
    }
    #[must_use]
    pub fn terminalize_failed() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("TerminalizeFailed")
    }
    #[must_use]
    pub fn terminalize_canceled() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("TerminalizeCanceled")
    }
}

pub mod signal {}

pub mod effect {
    #[must_use]
    pub fn emit_flow_run_notice() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("EmitFlowRunNotice")
    }
    #[must_use]
    pub fn emit_step_notice() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("EmitStepNotice")
    }
    #[must_use]
    pub fn append_failure_ledger() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("AppendFailureLedger")
    }
    #[must_use]
    pub fn persist_step_output() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("PersistStepOutput")
    }
    #[must_use]
    pub fn admit_step_work() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("AdmitStepWork")
    }
    #[must_use]
    pub fn flow_terminalized() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("FlowTerminalized")
    }
    #[must_use]
    pub fn escalate_supervisor() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("EscalateSupervisor")
    }
    #[must_use]
    pub fn project_target_success() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("ProjectTargetSuccess")
    }
    #[must_use]
    pub fn project_target_failure() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("ProjectTargetFailure")
    }
    #[must_use]
    pub fn project_target_canceled() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("ProjectTargetCanceled")
    }
    #[must_use]
    pub fn grant_node_slot() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("GrantNodeSlot")
    }
    #[must_use]
    pub fn grant_body_frame_start() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("GrantBodyFrameStart")
    }
}

pub mod helper {
    #[must_use]
    pub fn run_is_terminal() -> super::KernelHelperName {
        super::KernelHelperName::new_static("RunIsTerminal")
    }
    #[must_use]
    pub fn step_is_tracked() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepIsTracked")
    }
    #[must_use]
    pub fn step_status_is() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepStatusIs")
    }
    #[must_use]
    pub fn step_output_recorded_is() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepOutputRecordedIs")
    }
    #[must_use]
    pub fn step_condition_recorded_is() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepConditionRecordedIs")
    }
    #[must_use]
    pub fn step_condition_allows_dispatch() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepConditionAllowsDispatch")
    }
    #[must_use]
    pub fn all_tracked_steps_in_allowed_statuses() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AllTrackedStepsInAllowedStatuses")
    }
    #[must_use]
    pub fn no_tracked_step_in_status() -> super::KernelHelperName {
        super::KernelHelperName::new_static("NoTrackedStepInStatus")
    }
    #[must_use]
    pub fn any_tracked_step_in_status() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AnyTrackedStepInStatus")
    }
    #[must_use]
    pub fn step_has_dependencies() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepHasDependencies")
    }
    #[must_use]
    pub fn all_dependencies_completed() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AllDependenciesCompleted")
    }
    #[must_use]
    pub fn all_dependencies_skipped() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AllDependenciesSkipped")
    }
    #[must_use]
    pub fn any_dependency_completed() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AnyDependencyCompleted")
    }
    #[must_use]
    pub fn step_dependency_ready() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepDependencyReady")
    }
    #[must_use]
    pub fn step_dependency_should_skip() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepDependencyShouldSkip")
    }
    #[must_use]
    pub fn step_branch_blocked() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepBranchBlocked")
    }
    #[must_use]
    pub fn escalation_will_trigger() -> super::KernelHelperName {
        super::KernelHelperName::new_static("EscalationWillTrigger")
    }
    #[must_use]
    pub fn target_retry_count() -> super::KernelHelperName {
        super::KernelHelperName::new_static("TargetRetryCount")
    }
    #[must_use]
    pub fn target_retry_allowed() -> super::KernelHelperName {
        super::KernelHelperName::new_static("TargetRetryAllowed")
    }
    #[must_use]
    pub fn collection_satisfied() -> super::KernelHelperName {
        super::KernelHelperName::new_static("CollectionSatisfied")
    }
    #[must_use]
    pub fn collection_feasible() -> super::KernelHelperName {
        super::KernelHelperName::new_static("CollectionFeasible")
    }
    #[must_use]
    pub fn step_target_count() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepTargetCount")
    }
    #[must_use]
    pub fn step_target_success_count() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepTargetSuccessCount")
    }
    #[must_use]
    pub fn step_target_terminal_failure_count() -> super::KernelHelperName {
        super::KernelHelperName::new_static("StepTargetTerminalFailureCount")
    }
    #[must_use]
    pub fn remaining_target_count() -> super::KernelHelperName {
        super::KernelHelperName::new_static("RemainingTargetCount")
    }
}

pub mod transition {
    #[must_use]
    pub fn create_run() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CreateRun")
    }
    #[must_use]
    pub fn start_run() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("StartRun")
    }
    #[must_use]
    pub fn dispatch_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("DispatchStep")
    }
    #[must_use]
    pub fn complete_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CompleteStep")
    }
    #[must_use]
    pub fn record_step_output() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RecordStepOutput")
    }
    #[must_use]
    pub fn condition_passed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ConditionPassed")
    }
    #[must_use]
    pub fn condition_rejected() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ConditionRejected")
    }
    #[must_use]
    pub fn fail_step_escalating() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("FailStepEscalating")
    }
    #[must_use]
    pub fn fail_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("FailStep")
    }
    #[must_use]
    pub fn skip_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SkipStep")
    }
    #[must_use]
    pub fn project_frame_step_completed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ProjectFrameStepCompleted")
    }
    #[must_use]
    pub fn project_frame_step_skipped() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ProjectFrameStepSkipped")
    }
    #[must_use]
    pub fn project_frame_step_failed_escalating_with_ledger() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ProjectFrameStepFailedEscalatingWithLedger")
    }
    #[must_use]
    pub fn project_frame_step_failed_escalating_without_ledger() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ProjectFrameStepFailedEscalatingWithoutLedger")
    }
    #[must_use]
    pub fn project_frame_step_failed_with_ledger() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ProjectFrameStepFailedWithLedger")
    }
    #[must_use]
    pub fn project_frame_step_failed_without_ledger() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("ProjectFrameStepFailedWithoutLedger")
    }
    #[must_use]
    pub fn cancel_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CancelStep")
    }
    #[must_use]
    pub fn register_targets() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RegisterTargets")
    }
    #[must_use]
    pub fn record_target_success() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RecordTargetSuccess")
    }
    #[must_use]
    pub fn record_target_terminal_failure() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RecordTargetTerminalFailure")
    }
    #[must_use]
    pub fn record_target_canceled() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RecordTargetCanceled")
    }
    #[must_use]
    pub fn record_target_failure() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RecordTargetFailure")
    }
    #[must_use]
    pub fn register_ready_frame() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RegisterReadyFrame")
    }
    #[must_use]
    pub fn pump_node_scheduler() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("PumpNodeScheduler")
    }
    #[must_use]
    pub fn register_pending_body_frame() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RegisterPendingBodyFrame")
    }
    #[must_use]
    pub fn pump_frame_scheduler() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("PumpFrameScheduler")
    }
    #[must_use]
    pub fn node_execution_released() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("NodeExecutionReleased")
    }
    #[must_use]
    pub fn frame_terminated() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("FrameTerminated")
    }
    #[must_use]
    pub fn terminalize_completed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("TerminalizeCompleted")
    }
    #[must_use]
    pub fn terminalize_failed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("TerminalizeFailed")
    }
    #[must_use]
    pub fn terminalize_canceled() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("TerminalizeCanceled")
    }
}

pub fn initial_state() -> Result<KernelState, TransitionRefusal> {
    kernel().initial_state()
}

pub fn transition(
    state: &KernelState,
    input: &KernelInput,
) -> Result<TransitionOutcome, TransitionRefusal> {
    kernel().transition(state, input)
}

pub fn transition_signal(
    state: &KernelState,
    signal: &KernelSignal,
) -> Result<TransitionOutcome, TransitionRefusal> {
    kernel().transition_signal(state, signal)
}

pub fn evaluate_helper(
    state: &KernelState,
    helper_name: &KernelHelperName,
    args: &KernelFields,
) -> Result<KernelValue, TransitionRefusal> {
    kernel().evaluate_helper(state, helper_name, args)
}
