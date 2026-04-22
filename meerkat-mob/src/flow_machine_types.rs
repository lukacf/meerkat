use crate::definition::{CollectionPolicy, DependencyMode};
use crate::ids::{BranchId, FlowNodeId, FrameId, LoopId, LoopInstanceId, MeerkatId, StepId};
use crate::run::StepRunStatus;
use meerkat_machine_schema::compat::types as kernel_types;

pub(crate) fn step_id(value: &StepId) -> kernel_types::StepId {
    kernel_types::StepId::from(value.to_string())
}

pub(crate) fn local_step_id(value: &kernel_types::StepId) -> StepId {
    StepId::from(value.to_string())
}

pub(crate) fn frame_id(value: &FrameId) -> kernel_types::FrameId {
    kernel_types::FrameId::from(value.to_string())
}

pub(crate) fn local_frame_id(value: &kernel_types::FrameId) -> FrameId {
    FrameId::from(value.to_string())
}

pub(crate) fn loop_instance_id(value: &LoopInstanceId) -> kernel_types::LoopInstanceId {
    kernel_types::LoopInstanceId::from(value.to_string())
}

pub(crate) fn local_loop_instance_id(value: &kernel_types::LoopInstanceId) -> LoopInstanceId {
    LoopInstanceId::from(value.to_string())
}

pub(crate) fn flow_node_id(value: &FlowNodeId) -> kernel_types::FlowNodeId {
    kernel_types::FlowNodeId::from(value.to_string())
}

pub(crate) fn local_flow_node_id(value: &kernel_types::FlowNodeId) -> FlowNodeId {
    FlowNodeId::from(value.to_string())
}

pub(crate) fn loop_id(value: &LoopId) -> kernel_types::LoopId {
    kernel_types::LoopId::from(value.to_string())
}

pub(crate) fn local_loop_id(value: &kernel_types::LoopId) -> LoopId {
    LoopId::from(value.to_string())
}

pub(crate) fn branch_id(value: &BranchId) -> kernel_types::BranchId {
    kernel_types::BranchId::from(value.to_string())
}

pub(crate) fn local_branch_id(value: &kernel_types::BranchId) -> BranchId {
    BranchId::from(value.to_string())
}

pub(crate) fn local_target_id(value: &kernel_types::MeerkatId) -> MeerkatId {
    MeerkatId::from(value.to_string())
}

pub(crate) fn dependency_mode(value: DependencyMode) -> kernel_types::DependencyMode {
    match value {
        DependencyMode::All => kernel_types::DependencyMode::All,
        DependencyMode::Any => kernel_types::DependencyMode::Any,
    }
}

pub(crate) fn local_dependency_mode(value: kernel_types::DependencyMode) -> DependencyMode {
    match value {
        kernel_types::DependencyMode::All => DependencyMode::All,
        kernel_types::DependencyMode::Any => DependencyMode::Any,
    }
}

pub(crate) fn collection_policy_kind(
    value: &CollectionPolicy,
) -> kernel_types::CollectionPolicyKind {
    match value {
        CollectionPolicy::All => kernel_types::CollectionPolicyKind::All,
        CollectionPolicy::Any => kernel_types::CollectionPolicyKind::Any,
        CollectionPolicy::Quorum { .. } => kernel_types::CollectionPolicyKind::Quorum,
    }
}

pub(crate) fn local_step_run_status(value: kernel_types::StepRunStatus) -> StepRunStatus {
    match value {
        kernel_types::StepRunStatus::Dispatched => StepRunStatus::Dispatched,
        kernel_types::StepRunStatus::Completed => StepRunStatus::Completed,
        kernel_types::StepRunStatus::Failed => StepRunStatus::Failed,
        kernel_types::StepRunStatus::Skipped => StepRunStatus::Skipped,
        kernel_types::StepRunStatus::Canceled => StepRunStatus::Canceled,
    }
}
