// Local flow-frame kernel wrapper derived from the former compat codegen.
#![allow(dead_code)]
use meerkat_machine_kernels::{
    GeneratedMachineKernel, KernelEffectVariant, KernelField, KernelFields, KernelHelperName,
    KernelInput, KernelInputVariant, KernelPhase, KernelSignal, KernelSignalVariant, KernelState,
    KernelTransitionName, KernelValue, TransitionOutcome, TransitionRefusal,
};

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    super::flow_frame_machine_schema::flow_frame_machine()
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
    pub fn frame_id() -> super::KernelField {
        super::KernelField::new_static("frame_id")
    }
    #[must_use]
    pub fn frame_scope() -> super::KernelField {
        super::KernelField::new_static("frame_scope")
    }
    #[must_use]
    pub fn loop_instance_id() -> super::KernelField {
        super::KernelField::new_static("loop_instance_id")
    }
    #[must_use]
    pub fn iteration() -> super::KernelField {
        super::KernelField::new_static("iteration")
    }
    #[must_use]
    pub fn last_admitted_node() -> super::KernelField {
        super::KernelField::new_static("last_admitted_node")
    }
    #[must_use]
    pub fn tracked_nodes() -> super::KernelField {
        super::KernelField::new_static("tracked_nodes")
    }
    #[must_use]
    pub fn ordered_nodes() -> super::KernelField {
        super::KernelField::new_static("ordered_nodes")
    }
    #[must_use]
    pub fn node_kind() -> super::KernelField {
        super::KernelField::new_static("node_kind")
    }
    #[must_use]
    pub fn node_dependencies() -> super::KernelField {
        super::KernelField::new_static("node_dependencies")
    }
    #[must_use]
    pub fn node_dependency_modes() -> super::KernelField {
        super::KernelField::new_static("node_dependency_modes")
    }
    #[must_use]
    pub fn node_branches() -> super::KernelField {
        super::KernelField::new_static("node_branches")
    }
    #[must_use]
    pub fn branch_winners() -> super::KernelField {
        super::KernelField::new_static("branch_winners")
    }
    #[must_use]
    pub fn node_status() -> super::KernelField {
        super::KernelField::new_static("node_status")
    }
    #[must_use]
    pub fn ready_queue() -> super::KernelField {
        super::KernelField::new_static("ready_queue")
    }
    #[must_use]
    pub fn output_recorded() -> super::KernelField {
        super::KernelField::new_static("output_recorded")
    }
    #[must_use]
    pub fn node_condition_results() -> super::KernelField {
        super::KernelField::new_static("node_condition_results")
    }
    #[must_use]
    pub fn node_id() -> super::KernelField {
        super::KernelField::new_static("node_id")
    }
}

pub mod input {
    #[must_use]
    pub fn start_root_frame() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("StartRootFrame")
    }
    #[must_use]
    pub fn start_body_frame() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("StartBodyFrame")
    }
    #[must_use]
    pub fn admit_next_ready_node() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("AdmitNextReadyNode")
    }
    #[must_use]
    pub fn complete_node() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("CompleteNode")
    }
    #[must_use]
    pub fn record_node_output() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("RecordNodeOutput")
    }
    #[must_use]
    pub fn fail_node() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("FailNode")
    }
    #[must_use]
    pub fn skip_node() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("SkipNode")
    }
    #[must_use]
    pub fn cancel_node() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("CancelNode")
    }
    #[must_use]
    pub fn seal_frame() -> super::KernelInputVariant {
        super::KernelInputVariant::new_static("SealFrame")
    }
}

pub mod signal {}

pub mod effect {
    #[must_use]
    pub fn ready_frontier_changed() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("ReadyFrontierChanged")
    }
    #[must_use]
    pub fn admit_step_work() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("AdmitStepWork")
    }
    #[must_use]
    pub fn start_loop_node() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("StartLoopNode")
    }
    #[must_use]
    pub fn persist_step_output() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("PersistStepOutput")
    }
    #[must_use]
    pub fn node_execution_released() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("NodeExecutionReleased")
    }
    #[must_use]
    pub fn root_frame_completed() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("RootFrameCompleted")
    }
    #[must_use]
    pub fn root_frame_failed() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("RootFrameFailed")
    }
    #[must_use]
    pub fn root_frame_canceled() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("RootFrameCanceled")
    }
    #[must_use]
    pub fn body_frame_completed() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("BodyFrameCompleted")
    }
    #[must_use]
    pub fn body_frame_failed() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("BodyFrameFailed")
    }
    #[must_use]
    pub fn body_frame_canceled() -> super::KernelEffectVariant {
        super::KernelEffectVariant::new_static("BodyFrameCanceled")
    }
}

pub mod helper {
    #[must_use]
    pub fn node_admission_eligible() -> super::KernelHelperName {
        super::KernelHelperName::new_static("NodeAdmissionEligible")
    }
    #[must_use]
    pub fn all_deps_completed() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AllDepsCompleted")
    }
    #[must_use]
    pub fn any_dep_completed() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AnyDepCompleted")
    }
    #[must_use]
    pub fn all_nodes_terminal() -> super::KernelHelperName {
        super::KernelHelperName::new_static("AllNodesTerminal")
    }
}

pub mod transition {
    #[must_use]
    pub fn start_root_frame() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("StartRootFrame")
    }
    #[must_use]
    pub fn start_body_frame() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("StartBodyFrame")
    }
    #[must_use]
    pub fn admit_next_ready_node_step_run() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("AdmitNextReadyNode_StepRun")
    }
    #[must_use]
    pub fn admit_next_ready_node_loop_run() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("AdmitNextReadyNode_LoopRun")
    }
    #[must_use]
    pub fn admit_next_ready_node_skip() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("AdmitNextReadyNode_Skip")
    }
    #[must_use]
    pub fn admit_next_ready_node_fail() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("AdmitNextReadyNode_Fail")
    }
    #[must_use]
    pub fn complete_node_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CompleteNode_Step")
    }
    #[must_use]
    pub fn complete_node_loop() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CompleteNode_Loop")
    }
    #[must_use]
    pub fn record_node_output() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("RecordNodeOutput")
    }
    #[must_use]
    pub fn fail_node_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("FailNode_Step")
    }
    #[must_use]
    pub fn fail_node_loop() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("FailNode_Loop")
    }
    #[must_use]
    pub fn skip_node_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SkipNode_Step")
    }
    #[must_use]
    pub fn skip_node_loop() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SkipNode_Loop")
    }
    #[must_use]
    pub fn cancel_node_step() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CancelNode_Step")
    }
    #[must_use]
    pub fn cancel_node_loop() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("CancelNode_Loop")
    }
    #[must_use]
    pub fn seal_root_frame_canceled() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SealRootFrameCanceled")
    }
    #[must_use]
    pub fn seal_root_frame_failed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SealRootFrameFailed")
    }
    #[must_use]
    pub fn seal_root_frame_completed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SealRootFrameCompleted")
    }
    #[must_use]
    pub fn seal_body_frame_canceled() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SealBodyFrameCanceled")
    }
    #[must_use]
    pub fn seal_body_frame_failed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SealBodyFrameFailed")
    }
    #[must_use]
    pub fn seal_body_frame_completed() -> super::KernelTransitionName {
        super::KernelTransitionName::new_static("SealBodyFrameCompleted")
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
