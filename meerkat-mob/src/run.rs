//! Flow run data model and MobMachine-owned runtime projections.

use crate::MobMachineCatalogInput;
use crate::definition::{
    DependencyMode, FlowNodeSpec, FlowSpec, FrameSpec, LimitsSpec, SupervisorSpec, TopologySpec,
};
use crate::error::MobError;
use crate::ids::{
    AgentIdentity, BranchId, FlowId, FlowNodeId, FrameId, LoopId, LoopInstanceId, MeerkatId, MobId,
    ProfileName, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use meerkat_machine_schema::catalog::dsl::mob_machine::MobMachineInputVariant;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub mod flow_frame;
pub mod flow_run;
pub mod loop_iteration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowProjectionKernelRole {
    MobMachineOwnedFailClosedProjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowProjectionKernelAudit {
    pub module: &'static str,
    pub canonical_owner: &'static str,
    pub role: FlowProjectionKernelRole,
    pub canonical_machine: bool,
    pub owning_inputs: &'static [MobMachineCatalogInput],
}

const FLOW_RUN_OWNING_INPUTS: &[MobMachineCatalogInput] = &[
    MobMachineCatalogInput::CreateRunSeed,
    MobMachineCatalogInput::AuthorizeFlowRunReducerCommand,
];
const FLOW_FRAME_OWNING_INPUTS: &[MobMachineCatalogInput] = &[
    MobMachineCatalogInput::CreateFrameSeed,
    MobMachineCatalogInput::AuthorizeFlowFrameReducerCommand,
];
const LOOP_ITERATION_OWNING_INPUTS: &[MobMachineCatalogInput] = &[
    MobMachineCatalogInput::CreateLoopSeed,
    MobMachineCatalogInput::RecordLoopBodyFrameCompleted,
    MobMachineCatalogInput::RecordLoopUntilConditionMet,
    MobMachineCatalogInput::RecordLoopUntilConditionFailed,
    MobMachineCatalogInput::AuthorizeLoopIterationReducerCommand,
];

const FLOW_PROJECTION_KERNEL_AUDIT: &[FlowProjectionKernelAudit] = &[
    FlowProjectionKernelAudit {
        module: "flow_run",
        canonical_owner: "MobMachine",
        role: FlowProjectionKernelRole::MobMachineOwnedFailClosedProjection,
        canonical_machine: false,
        owning_inputs: FLOW_RUN_OWNING_INPUTS,
    },
    FlowProjectionKernelAudit {
        module: "flow_frame",
        canonical_owner: "MobMachine",
        role: FlowProjectionKernelRole::MobMachineOwnedFailClosedProjection,
        canonical_machine: false,
        owning_inputs: FLOW_FRAME_OWNING_INPUTS,
    },
    FlowProjectionKernelAudit {
        module: "loop_iteration",
        canonical_owner: "MobMachine",
        role: FlowProjectionKernelRole::MobMachineOwnedFailClosedProjection,
        canonical_machine: false,
        owning_inputs: LOOP_ITERATION_OWNING_INPUTS,
    },
];

pub fn flow_projection_kernel_audit() -> &'static [FlowProjectionKernelAudit] {
    FLOW_PROJECTION_KERNEL_AUDIT
}

#[must_use]
pub fn canonical_flow_authority_input_manifest() -> indexmap::IndexSet<&'static str> {
    canonical_flow_authority_input_variant_manifest()
        .into_iter()
        .map(|variant| variant.as_str())
        .collect()
}

#[must_use]
pub fn canonical_flow_authority_input_variant_manifest()
-> indexmap::IndexSet<MobMachineInputVariant> {
    std::iter::once(MobMachineCatalogInput::RunFlow)
        .chain(
            flow_projection_kernel_audit()
                .iter()
                .flat_map(|record| record.owning_inputs.iter().copied()),
        )
        .map(MobMachineCatalogInput::input_variant)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobMachineFlowAuthorityKind {
    FlowRun(mob_dsl::FlowRunReducerCommandKind),
    FlowFrame(mob_dsl::FlowFrameReducerCommandKind),
    LoopIteration(mob_dsl::LoopIterationReducerCommandKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobMachineFlowAuthoritySource {
    MachineOwnedInput(MobMachineCatalogInput),
    AuthorizationOnlyInput(MobMachineCatalogInput),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MobMachineFlowAuthorityToken {
    kind: MobMachineFlowAuthorityKind,
    source: MobMachineFlowAuthoritySource,
}

macro_rules! non_flow_reducer_authority_mob_machine_inputs {
    () => {
        mob_dsl::MobMachineInput::RunFlow { .. }
            | mob_dsl::MobMachineInput::CancelFlow { .. }
            | mob_dsl::MobMachineInput::FlowStatus
            | mob_dsl::MobMachineInput::Spawn { .. }
            | mob_dsl::MobMachineInput::EnsureMember { .. }
            | mob_dsl::MobMachineInput::Reconcile { .. }
            | mob_dsl::MobMachineInput::Retire { .. }
            | mob_dsl::MobMachineInput::Respawn { .. }
            | mob_dsl::MobMachineInput::RetireAll
            | mob_dsl::MobMachineInput::WireMembers { .. }
            | mob_dsl::MobMachineInput::UnwireMembers { .. }
            | mob_dsl::MobMachineInput::WireExternalPeer { .. }
            | mob_dsl::MobMachineInput::UnwireExternalPeer { .. }
            | mob_dsl::MobMachineInput::SubmitWork { .. }
            | mob_dsl::MobMachineInput::CancelWork { .. }
            | mob_dsl::MobMachineInput::CancelAllWork { .. }
            | mob_dsl::MobMachineInput::Stop
            | mob_dsl::MobMachineInput::Resume
            | mob_dsl::MobMachineInput::Complete
            | mob_dsl::MobMachineInput::Reset
            | mob_dsl::MobMachineInput::Destroy
            | mob_dsl::MobMachineInput::TaskCreate { .. }
            | mob_dsl::MobMachineInput::TaskUpdate { .. }
            | mob_dsl::MobMachineInput::TaskList
            | mob_dsl::MobMachineInput::TaskGet
            | mob_dsl::MobMachineInput::RosterSnapshot
            | mob_dsl::MobMachineInput::ListMembers
            | mob_dsl::MobMachineInput::ListMembersIncludingRetiring
            | mob_dsl::MobMachineInput::ListAllMembers
            | mob_dsl::MobMachineInput::MemberStatus
            | mob_dsl::MobMachineInput::SubscribeAgentEvents
            | mob_dsl::MobMachineInput::SubscribeAllAgentEvents
            | mob_dsl::MobMachineInput::SubscribeMobEvents
            | mob_dsl::MobMachineInput::PollEvents
            | mob_dsl::MobMachineInput::ReplayAllEvents
            | mob_dsl::MobMachineInput::RecordOperatorActionProvenance
            | mob_dsl::MobMachineInput::GetMember
            | mob_dsl::MobMachineInput::SetSpawnPolicy
            | mob_dsl::MobMachineInput::Shutdown
            | mob_dsl::MobMachineInput::ForceCancel
            | mob_dsl::MobMachineInput::KickoffMarkPending { .. }
            | mob_dsl::MobMachineInput::KickoffMarkStarting { .. }
            | mob_dsl::MobMachineInput::StartupMarkReady { .. }
            | mob_dsl::MobMachineInput::KickoffResolveStarted { .. }
            | mob_dsl::MobMachineInput::KickoffResolveCallbackPending { .. }
            | mob_dsl::MobMachineInput::KickoffResolveFailed { .. }
            | mob_dsl::MobMachineInput::KickoffCancelRequested { .. }
            | mob_dsl::MobMachineInput::KickoffClear { .. }
    };
}

impl MobMachineFlowAuthorityToken {
    pub(crate) fn from_accepted_mob_machine_input(
        input: &mob_dsl::MobMachineInput,
    ) -> Result<Self, MobError> {
        let catalog_input = input_catalog(input)?;
        let input_variant = catalog_input.input_variant();
        if !canonical_flow_authority_input_variant_manifest().contains(&input_variant) {
            return Err(MobError::Internal(format!(
                "MobMachine input variant {input_variant:?} is not in the typed flow authority manifest"
            )));
        }

        match input {
            mob_dsl::MobMachineInput::CreateRunSeed { .. } => Ok(Self::new(
                MobMachineFlowAuthorityKind::FlowRun(mob_dsl::FlowRunReducerCommandKind::CreateRun),
                MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input),
            )),
            mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand { command, .. } => {
                let source = match command {
                    mob_dsl::FlowRunReducerCommandKind::CreateRun => {
                        MobMachineFlowAuthoritySource::AuthorizationOnlyInput(catalog_input)
                    }
                    mob_dsl::FlowRunReducerCommandKind::StartRun
                    | mob_dsl::FlowRunReducerCommandKind::DispatchStep
                    | mob_dsl::FlowRunReducerCommandKind::CompleteStep
                    | mob_dsl::FlowRunReducerCommandKind::RecordStepOutput
                    | mob_dsl::FlowRunReducerCommandKind::ConditionPassed
                    | mob_dsl::FlowRunReducerCommandKind::ConditionRejected
                    | mob_dsl::FlowRunReducerCommandKind::FailStep
                    | mob_dsl::FlowRunReducerCommandKind::SkipStep
                    | mob_dsl::FlowRunReducerCommandKind::ProjectFrameStepStatus
                    | mob_dsl::FlowRunReducerCommandKind::CancelStep
                    | mob_dsl::FlowRunReducerCommandKind::RegisterTargets
                    | mob_dsl::FlowRunReducerCommandKind::RecordTargetSuccess
                    | mob_dsl::FlowRunReducerCommandKind::RecordTargetTerminalFailure
                    | mob_dsl::FlowRunReducerCommandKind::RecordTargetCanceled
                    | mob_dsl::FlowRunReducerCommandKind::RecordTargetFailure
                    | mob_dsl::FlowRunReducerCommandKind::RegisterReadyFrame
                    | mob_dsl::FlowRunReducerCommandKind::PumpNodeScheduler
                    | mob_dsl::FlowRunReducerCommandKind::RegisterPendingBodyFrame
                    | mob_dsl::FlowRunReducerCommandKind::PumpFrameScheduler
                    | mob_dsl::FlowRunReducerCommandKind::NodeExecutionReleased
                    | mob_dsl::FlowRunReducerCommandKind::FrameTerminated
                    | mob_dsl::FlowRunReducerCommandKind::TerminalizeCompleted
                    | mob_dsl::FlowRunReducerCommandKind::TerminalizeFailed
                    | mob_dsl::FlowRunReducerCommandKind::TerminalizeCanceled => {
                        MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input)
                    }
                };
                Ok(Self::new(
                    MobMachineFlowAuthorityKind::FlowRun(*command),
                    source,
                ))
            }
            mob_dsl::MobMachineInput::CreateFrameSeed { frame_scope, .. } => Ok(Self::new(
                MobMachineFlowAuthorityKind::FlowFrame(match frame_scope {
                    mob_dsl::FrameScope::Root => {
                        mob_dsl::FlowFrameReducerCommandKind::StartRootFrame
                    }
                    mob_dsl::FrameScope::Body => {
                        mob_dsl::FlowFrameReducerCommandKind::StartBodyFrame
                    }
                }),
                MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input),
            )),
            mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand { command, .. } => {
                let source = match command {
                    mob_dsl::FlowFrameReducerCommandKind::StartRootFrame
                    | mob_dsl::FlowFrameReducerCommandKind::StartBodyFrame => {
                        MobMachineFlowAuthoritySource::AuthorizationOnlyInput(catalog_input)
                    }
                    mob_dsl::FlowFrameReducerCommandKind::AdmitNextReadyNode
                    | mob_dsl::FlowFrameReducerCommandKind::CompleteNode
                    | mob_dsl::FlowFrameReducerCommandKind::RecordNodeOutput
                    | mob_dsl::FlowFrameReducerCommandKind::FailNode
                    | mob_dsl::FlowFrameReducerCommandKind::SkipNode
                    | mob_dsl::FlowFrameReducerCommandKind::CancelNode
                    | mob_dsl::FlowFrameReducerCommandKind::SealFrame => {
                        MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input)
                    }
                };
                Ok(Self::new(
                    MobMachineFlowAuthorityKind::FlowFrame(*command),
                    source,
                ))
            }
            mob_dsl::MobMachineInput::CreateLoopSeed { .. } => Ok(Self::new(
                MobMachineFlowAuthorityKind::LoopIteration(
                    mob_dsl::LoopIterationReducerCommandKind::StartLoop,
                ),
                MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input),
            )),
            mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted { .. } => Ok(Self::new(
                MobMachineFlowAuthorityKind::LoopIteration(
                    mob_dsl::LoopIterationReducerCommandKind::BodyFrameCompleted,
                ),
                MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input),
            )),
            mob_dsl::MobMachineInput::RecordLoopUntilConditionMet { .. } => Ok(Self::new(
                MobMachineFlowAuthorityKind::LoopIteration(
                    mob_dsl::LoopIterationReducerCommandKind::UntilConditionMet,
                ),
                MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input),
            )),
            mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed { .. } => Ok(Self::new(
                MobMachineFlowAuthorityKind::LoopIteration(
                    mob_dsl::LoopIterationReducerCommandKind::UntilConditionFailed,
                ),
                MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input),
            )),
            mob_dsl::MobMachineInput::AuthorizeLoopIterationReducerCommand { command, .. } => {
                let source = match command {
                    mob_dsl::LoopIterationReducerCommandKind::StartLoop
                    | mob_dsl::LoopIterationReducerCommandKind::BodyFrameStarted
                    | mob_dsl::LoopIterationReducerCommandKind::BodyFrameCompleted
                    | mob_dsl::LoopIterationReducerCommandKind::UntilConditionMet
                    | mob_dsl::LoopIterationReducerCommandKind::UntilConditionFailed => {
                        MobMachineFlowAuthoritySource::AuthorizationOnlyInput(catalog_input)
                    }
                    mob_dsl::LoopIterationReducerCommandKind::BodyFrameFailed
                    | mob_dsl::LoopIterationReducerCommandKind::BodyFrameCanceled
                    | mob_dsl::LoopIterationReducerCommandKind::CancelLoop => {
                        MobMachineFlowAuthoritySource::MachineOwnedInput(catalog_input)
                    }
                };
                Ok(Self::new(
                    MobMachineFlowAuthorityKind::LoopIteration(*command),
                    source,
                ))
            }
            crate::mob_destroying_session_ingress_feedback_input_patterns!() => {
                Err(MobError::Internal(format!(
                    "MobMachine input {input:?} is not a flow reducer authority input"
                )))
            }
            non_flow_reducer_authority_mob_machine_inputs!() => Err(MobError::Internal(format!(
                "MobMachine input {input:?} is not a flow reducer authority input"
            ))),
        }
    }

    pub(crate) fn from_accepted_mob_machine_body_frame_seed(
        input: &mob_dsl::MobMachineInput,
    ) -> Result<Self, MobError> {
        match input {
            mob_dsl::MobMachineInput::CreateFrameSeed {
                frame_scope: mob_dsl::FrameScope::Body,
                ..
            } => Ok(Self::new(
                MobMachineFlowAuthorityKind::LoopIteration(
                    mob_dsl::LoopIterationReducerCommandKind::BodyFrameStarted,
                ),
                MobMachineFlowAuthoritySource::MachineOwnedInput(input_catalog(input)?),
            )),
            crate::mob_destroying_session_ingress_feedback_input_patterns!() => {
                Err(MobError::Internal(format!(
                    "MobMachine input {input:?} is not a body-frame seed authority input"
                )))
            }
            non_flow_reducer_authority_mob_machine_inputs!() => Err(MobError::Internal(format!(
                "MobMachine input {input:?} is not a body-frame seed authority input"
            ))),
            mob_dsl::MobMachineInput::CreateFrameSeed {
                frame_scope: mob_dsl::FrameScope::Root,
                ..
            }
            | mob_dsl::MobMachineInput::CreateRunSeed { .. }
            | mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand { .. }
            | mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand { .. }
            | mob_dsl::MobMachineInput::CreateLoopSeed { .. }
            | mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted { .. }
            | mob_dsl::MobMachineInput::RecordLoopUntilConditionMet { .. }
            | mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed { .. }
            | mob_dsl::MobMachineInput::AuthorizeLoopIterationReducerCommand { .. } => {
                Err(MobError::Internal(format!(
                    "MobMachine input {input:?} is not a body-frame seed authority input"
                )))
            }
        }
    }

    fn new(kind: MobMachineFlowAuthorityKind, source: MobMachineFlowAuthoritySource) -> Self {
        Self { kind, source }
    }

    fn require(self, expected: MobMachineFlowAuthorityKind) -> Result<(), MobError> {
        if self.kind == expected {
            match self.source {
                MobMachineFlowAuthoritySource::MachineOwnedInput(_) => Ok(()),
                MobMachineFlowAuthoritySource::AuthorizationOnlyInput(input) => {
                    Err(MobError::Internal(format!(
                        "MobMachine input {input:?} only authorized {:?}; reducer-visible state \
                         changes for {:?} must be machine-owned and are fail-closed",
                        self.kind, expected
                    )))
                }
            }
        } else {
            Err(MobError::Internal(format!(
                "MobMachine flow authority token kind {:?} from {:?} cannot authorize {:?} reducer",
                self.kind, self.source, expected
            )))
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum MobMachineFlowRunCommand {
    CreateRun(flow_run::inputs::CreateRun),
    StartRun(flow_run::inputs::StartRun),
    DispatchStep(flow_run::inputs::DispatchStep),
    CompleteStep(flow_run::inputs::CompleteStep),
    RecordStepOutput(flow_run::inputs::RecordStepOutput),
    ConditionPassed(flow_run::inputs::ConditionPassed),
    ConditionRejected(flow_run::inputs::ConditionRejected),
    FailStep(flow_run::inputs::FailStep),
    SkipStep(flow_run::inputs::SkipStep),
    ProjectFrameStepStatus(flow_run::inputs::ProjectFrameStepStatus),
    CancelStep(flow_run::inputs::CancelStep),
    RegisterTargets(flow_run::inputs::RegisterTargets),
    RecordTargetSuccess(flow_run::inputs::RecordTargetSuccess),
    RecordTargetTerminalFailure(flow_run::inputs::RecordTargetTerminalFailure),
    RecordTargetCanceled(flow_run::inputs::RecordTargetCanceled),
    RecordTargetFailure(flow_run::inputs::RecordTargetFailure),
    RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame),
    PumpNodeScheduler(flow_run::inputs::PumpNodeScheduler),
    RegisterPendingBodyFrame(flow_run::inputs::RegisterPendingBodyFrame),
    PumpFrameScheduler(flow_run::inputs::PumpFrameScheduler),
    NodeExecutionReleased(flow_run::inputs::NodeExecutionReleased),
    FrameTerminated(flow_run::inputs::FrameTerminated),
    TerminalizeCompleted(flow_run::inputs::TerminalizeCompleted),
    TerminalizeFailed(flow_run::inputs::TerminalizeFailed),
    TerminalizeCanceled(flow_run::inputs::TerminalizeCanceled),
}

impl MobMachineFlowRunCommand {
    pub(crate) fn authority_input(&self, run_id: &RunId) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            command: self.kind(),
            step_id: self
                .step_id()
                .map(|step_id| mob_dsl::StepId::from(step_id.as_str())),
            run_step_key: self.step_id().map(|step_id| {
                mob_dsl::RunStepKey::from(format!("{}\u{0}{}", run_id, step_id.as_str()))
            }),
            step_status: self.step_status(),
            target_count: self.target_count().map(u64::from),
            frame_id: self
                .frame_id()
                .map(|frame_id| mob_dsl::FrameId::from(frame_id.as_str())),
            node_id: self
                .node_id()
                .map(|node_id| mob_dsl::FlowNodeId::from(node_id.as_str())),
            loop_instance_id: self
                .loop_instance_id()
                .map(|loop_id| mob_dsl::LoopInstanceId::from(loop_id.as_str())),
            retry_key: self.retry_key().map(str::to_owned),
        }
    }

    pub(crate) fn kind(&self) -> mob_dsl::FlowRunReducerCommandKind {
        match self {
            Self::CreateRun(_) => mob_dsl::FlowRunReducerCommandKind::CreateRun,
            Self::StartRun(_) => mob_dsl::FlowRunReducerCommandKind::StartRun,
            Self::DispatchStep(_) => mob_dsl::FlowRunReducerCommandKind::DispatchStep,
            Self::CompleteStep(_) => mob_dsl::FlowRunReducerCommandKind::CompleteStep,
            Self::RecordStepOutput(_) => mob_dsl::FlowRunReducerCommandKind::RecordStepOutput,
            Self::ConditionPassed(_) => mob_dsl::FlowRunReducerCommandKind::ConditionPassed,
            Self::ConditionRejected(_) => mob_dsl::FlowRunReducerCommandKind::ConditionRejected,
            Self::FailStep(_) => mob_dsl::FlowRunReducerCommandKind::FailStep,
            Self::SkipStep(_) => mob_dsl::FlowRunReducerCommandKind::SkipStep,
            Self::ProjectFrameStepStatus(_) => {
                mob_dsl::FlowRunReducerCommandKind::ProjectFrameStepStatus
            }
            Self::CancelStep(_) => mob_dsl::FlowRunReducerCommandKind::CancelStep,
            Self::RegisterTargets(_) => mob_dsl::FlowRunReducerCommandKind::RegisterTargets,
            Self::RecordTargetSuccess(_) => mob_dsl::FlowRunReducerCommandKind::RecordTargetSuccess,
            Self::RecordTargetTerminalFailure(_) => {
                mob_dsl::FlowRunReducerCommandKind::RecordTargetTerminalFailure
            }
            Self::RecordTargetCanceled(_) => {
                mob_dsl::FlowRunReducerCommandKind::RecordTargetCanceled
            }
            Self::RecordTargetFailure(_) => mob_dsl::FlowRunReducerCommandKind::RecordTargetFailure,
            Self::RegisterReadyFrame(_) => mob_dsl::FlowRunReducerCommandKind::RegisterReadyFrame,
            Self::PumpNodeScheduler(_) => mob_dsl::FlowRunReducerCommandKind::PumpNodeScheduler,
            Self::RegisterPendingBodyFrame(_) => {
                mob_dsl::FlowRunReducerCommandKind::RegisterPendingBodyFrame
            }
            Self::PumpFrameScheduler(_) => mob_dsl::FlowRunReducerCommandKind::PumpFrameScheduler,
            Self::NodeExecutionReleased(_) => {
                mob_dsl::FlowRunReducerCommandKind::NodeExecutionReleased
            }
            Self::FrameTerminated(_) => mob_dsl::FlowRunReducerCommandKind::FrameTerminated,
            Self::TerminalizeCompleted(_) => {
                mob_dsl::FlowRunReducerCommandKind::TerminalizeCompleted
            }
            Self::TerminalizeFailed(_) => mob_dsl::FlowRunReducerCommandKind::TerminalizeFailed,
            Self::TerminalizeCanceled(_) => mob_dsl::FlowRunReducerCommandKind::TerminalizeCanceled,
        }
    }

    fn step_id(&self) -> Option<&StepId> {
        match self {
            Self::DispatchStep(payload) => Some(&payload.step_id),
            Self::CompleteStep(payload) => Some(&payload.step_id),
            Self::RecordStepOutput(payload) => Some(&payload.step_id),
            Self::ConditionPassed(payload) => Some(&payload.step_id),
            Self::ConditionRejected(payload) => Some(&payload.step_id),
            Self::FailStep(payload) => Some(&payload.step_id),
            Self::SkipStep(payload) => Some(&payload.step_id),
            Self::ProjectFrameStepStatus(payload) => Some(&payload.step_id),
            Self::CancelStep(payload) => Some(&payload.step_id),
            Self::RegisterTargets(payload) => Some(&payload.step_id),
            Self::RecordTargetSuccess(payload) => Some(&payload.step_id),
            Self::RecordTargetTerminalFailure(payload) => Some(&payload.step_id),
            Self::RecordTargetCanceled(payload) => Some(&payload.step_id),
            Self::RecordTargetFailure(payload) => Some(&payload.step_id),
            _ => None,
        }
    }

    fn step_status(&self) -> Option<mob_dsl::StepRunStatus> {
        let status = match self {
            Self::DispatchStep(_) => flow_run::StepRunStatus::Dispatched,
            Self::CompleteStep(_) => flow_run::StepRunStatus::Completed,
            Self::FailStep(_) => flow_run::StepRunStatus::Failed,
            Self::SkipStep(_) => flow_run::StepRunStatus::Skipped,
            Self::CancelStep(_) => flow_run::StepRunStatus::Canceled,
            _ => return None,
        };
        Some(match status {
            flow_run::StepRunStatus::Dispatched => mob_dsl::StepRunStatus::Dispatched,
            flow_run::StepRunStatus::Completed => mob_dsl::StepRunStatus::Completed,
            flow_run::StepRunStatus::Failed => mob_dsl::StepRunStatus::Failed,
            flow_run::StepRunStatus::Skipped => mob_dsl::StepRunStatus::Skipped,
            flow_run::StepRunStatus::Canceled => mob_dsl::StepRunStatus::Canceled,
        })
    }

    fn target_count(&self) -> Option<u32> {
        match self {
            Self::RegisterTargets(payload) => Some(payload.target_count),
            _ => None,
        }
    }

    fn frame_id(&self) -> Option<&FrameId> {
        match self {
            Self::ProjectFrameStepStatus(payload) => Some(&payload.frame_id),
            Self::RegisterReadyFrame(payload) => Some(&payload.frame_id),
            Self::PumpNodeScheduler(payload) => Some(&payload.frame_id),
            Self::NodeExecutionReleased(payload) => Some(&payload.frame_id),
            Self::FrameTerminated(payload) => Some(&payload.frame_id),
            _ => None,
        }
    }

    fn node_id(&self) -> Option<&FlowNodeId> {
        match self {
            Self::ProjectFrameStepStatus(payload) => Some(&payload.node_id),
            _ => None,
        }
    }

    fn loop_instance_id(&self) -> Option<&LoopInstanceId> {
        match self {
            Self::RegisterPendingBodyFrame(payload) => Some(&payload.loop_instance_id),
            Self::PumpFrameScheduler(payload) => Some(&payload.loop_instance_id),
            _ => None,
        }
    }

    fn retry_key(&self) -> Option<&str> {
        match self {
            Self::RecordTargetFailure(payload) => Some(payload.retry_key.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum MobMachineFlowFrameCommand {
    StartRootFrame(flow_frame::inputs::StartRootFrame),
    StartBodyFrame(flow_frame::inputs::StartBodyFrame),
    AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode),
    CompleteNode(flow_frame::inputs::CompleteNode),
    RecordNodeOutput(flow_frame::inputs::RecordNodeOutput),
    FailNode(flow_frame::inputs::FailNode),
    SkipNode(flow_frame::inputs::SkipNode),
    CancelNode(flow_frame::inputs::CancelNode),
    SealFrame(flow_frame::inputs::SealFrame),
}

impl MobMachineFlowFrameCommand {
    pub(crate) fn authority_input(&self, frame_id: &FrameId) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand {
            frame_id: mob_dsl::FrameId::from(frame_id.as_str()),
            command: self.kind(),
            node_id: self
                .node_id()
                .map(|node_id| mob_dsl::FlowNodeId::from(node_id.as_str())),
            frame_node_key: self.node_id().map(|node_id| {
                mob_dsl::FrameNodeKey::from(format!("{frame_id}\u{0}{}", node_id.as_str()))
            }),
            node_status: self.node_status(),
            terminal_status: self.terminal_status(),
        }
    }

    pub(crate) fn kind(&self) -> mob_dsl::FlowFrameReducerCommandKind {
        match self {
            Self::StartRootFrame(_) => mob_dsl::FlowFrameReducerCommandKind::StartRootFrame,
            Self::StartBodyFrame(_) => mob_dsl::FlowFrameReducerCommandKind::StartBodyFrame,
            Self::AdmitNextReadyNode(_) => mob_dsl::FlowFrameReducerCommandKind::AdmitNextReadyNode,
            Self::CompleteNode(_) => mob_dsl::FlowFrameReducerCommandKind::CompleteNode,
            Self::RecordNodeOutput(_) => mob_dsl::FlowFrameReducerCommandKind::RecordNodeOutput,
            Self::FailNode(_) => mob_dsl::FlowFrameReducerCommandKind::FailNode,
            Self::SkipNode(_) => mob_dsl::FlowFrameReducerCommandKind::SkipNode,
            Self::CancelNode(_) => mob_dsl::FlowFrameReducerCommandKind::CancelNode,
            Self::SealFrame(_) => mob_dsl::FlowFrameReducerCommandKind::SealFrame,
        }
    }

    fn node_id(&self) -> Option<&FlowNodeId> {
        match self {
            Self::AdmitNextReadyNode(payload) => Some(&payload.node_id),
            Self::CompleteNode(payload) => Some(&payload.node_id),
            Self::RecordNodeOutput(payload) => Some(&payload.node_id),
            Self::FailNode(payload) => Some(&payload.node_id),
            Self::SkipNode(payload) => Some(&payload.node_id),
            Self::CancelNode(payload) => Some(&payload.node_id),
            _ => None,
        }
    }

    fn node_status(&self) -> Option<mob_dsl::NodeRunStatus> {
        let status = match self {
            Self::AdmitNextReadyNode(_) => flow_frame::NodeRunStatus::Running,
            Self::CompleteNode(_) => flow_frame::NodeRunStatus::Completed,
            Self::FailNode(_) => flow_frame::NodeRunStatus::Failed,
            Self::SkipNode(_) => flow_frame::NodeRunStatus::Skipped,
            Self::CancelNode(_) => flow_frame::NodeRunStatus::Canceled,
            _ => return None,
        };
        Some(match status {
            flow_frame::NodeRunStatus::Pending => mob_dsl::NodeRunStatus::Pending,
            flow_frame::NodeRunStatus::Ready => mob_dsl::NodeRunStatus::Ready,
            flow_frame::NodeRunStatus::Running => mob_dsl::NodeRunStatus::Running,
            flow_frame::NodeRunStatus::Completed => mob_dsl::NodeRunStatus::Completed,
            flow_frame::NodeRunStatus::Failed => mob_dsl::NodeRunStatus::Failed,
            flow_frame::NodeRunStatus::Skipped => mob_dsl::NodeRunStatus::Skipped,
            flow_frame::NodeRunStatus::Canceled => mob_dsl::NodeRunStatus::Canceled,
        })
    }

    fn terminal_status(&self) -> Option<mob_dsl::FrameStatus> {
        match self {
            Self::SealFrame(payload) => Some(match payload.terminal_status {
                flow_frame::FrameTerminalStatus::Completed => mob_dsl::FrameStatus::Completed,
                flow_frame::FrameTerminalStatus::Failed => mob_dsl::FrameStatus::Failed,
                flow_frame::FrameTerminalStatus::Canceled => mob_dsl::FrameStatus::Canceled,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum MobMachineLoopIterationCommand {
    StartLoop(loop_iteration::inputs::StartLoop),
    BodyFrameStarted(loop_iteration::inputs::BodyFrameStarted),
    BodyFrameCompleted(loop_iteration::inputs::BodyFrameCompleted),
    BodyFrameFailed(loop_iteration::inputs::BodyFrameFailed),
    BodyFrameCanceled(loop_iteration::inputs::BodyFrameCanceled),
    UntilConditionMet(loop_iteration::inputs::UntilConditionMet),
    UntilConditionFailed(loop_iteration::inputs::UntilConditionFailed),
    CancelLoop(loop_iteration::inputs::CancelLoop),
}

impl MobMachineLoopIterationCommand {
    pub(crate) fn authority_input(
        &self,
        loop_instance_id: &LoopInstanceId,
    ) -> mob_dsl::MobMachineInput {
        let loop_instance_id = mob_dsl::LoopInstanceId::from(loop_instance_id.as_str());
        match self {
            Self::BodyFrameCompleted(payload) => {
                mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted {
                    loop_instance_id,
                    iteration: payload.iteration as u64,
                }
            }
            Self::UntilConditionMet(payload) => {
                mob_dsl::MobMachineInput::RecordLoopUntilConditionMet {
                    loop_instance_id,
                    iteration: payload.iteration as u64,
                }
            }
            Self::UntilConditionFailed(payload) => {
                mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed {
                    loop_instance_id,
                    iteration: payload.iteration as u64,
                }
            }
            _ => mob_dsl::MobMachineInput::AuthorizeLoopIterationReducerCommand {
                loop_instance_id,
                command: self.kind(),
                body_frame_id: self
                    .body_frame_id()
                    .map(|frame_id| mob_dsl::FrameId::from(frame_id.as_str())),
                body_frame_iteration: self.body_frame_iteration(),
            },
        }
    }

    pub(crate) fn kind(&self) -> mob_dsl::LoopIterationReducerCommandKind {
        match self {
            Self::StartLoop(_) => mob_dsl::LoopIterationReducerCommandKind::StartLoop,
            Self::BodyFrameStarted(_) => mob_dsl::LoopIterationReducerCommandKind::BodyFrameStarted,
            Self::BodyFrameCompleted(_) => {
                mob_dsl::LoopIterationReducerCommandKind::BodyFrameCompleted
            }
            Self::BodyFrameFailed(_) => mob_dsl::LoopIterationReducerCommandKind::BodyFrameFailed,
            Self::BodyFrameCanceled(_) => {
                mob_dsl::LoopIterationReducerCommandKind::BodyFrameCanceled
            }
            Self::UntilConditionMet(_) => {
                mob_dsl::LoopIterationReducerCommandKind::UntilConditionMet
            }
            Self::UntilConditionFailed(_) => {
                mob_dsl::LoopIterationReducerCommandKind::UntilConditionFailed
            }
            Self::CancelLoop(_) => mob_dsl::LoopIterationReducerCommandKind::CancelLoop,
        }
    }

    fn body_frame_iteration(&self) -> Option<u64> {
        match self {
            Self::BodyFrameStarted(payload) => Some(payload.iteration as u64),
            Self::BodyFrameCompleted(payload) => Some(payload.iteration as u64),
            Self::BodyFrameFailed(payload) => Some(payload.iteration as u64),
            Self::BodyFrameCanceled(payload) => Some(payload.iteration as u64),
            _ => None,
        }
    }

    fn body_frame_id(&self) -> Option<&FrameId> {
        match self {
            Self::BodyFrameStarted(payload) => Some(&payload.frame_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowRunReducerCommandRecord {
    CreateRun,
    StartRun,
    DispatchStep,
    CompleteStep,
    RecordStepOutput,
    ConditionPassed,
    ConditionRejected,
    FailStep,
    SkipStep,
    ProjectFrameStepStatus,
    CancelStep,
    RegisterTargets,
    RecordTargetSuccess,
    RecordTargetTerminalFailure,
    RecordTargetCanceled,
    RecordTargetFailure,
    RegisterReadyFrame,
    PumpNodeScheduler,
    RegisterPendingBodyFrame,
    PumpFrameScheduler,
    NodeExecutionReleased,
    FrameTerminated,
    TerminalizeCompleted,
    TerminalizeFailed,
    TerminalizeCanceled,
}

impl From<mob_dsl::FlowRunReducerCommandKind> for FlowRunReducerCommandRecord {
    fn from(kind: mob_dsl::FlowRunReducerCommandKind) -> Self {
        match kind {
            mob_dsl::FlowRunReducerCommandKind::CreateRun => Self::CreateRun,
            mob_dsl::FlowRunReducerCommandKind::StartRun => Self::StartRun,
            mob_dsl::FlowRunReducerCommandKind::DispatchStep => Self::DispatchStep,
            mob_dsl::FlowRunReducerCommandKind::CompleteStep => Self::CompleteStep,
            mob_dsl::FlowRunReducerCommandKind::RecordStepOutput => Self::RecordStepOutput,
            mob_dsl::FlowRunReducerCommandKind::ConditionPassed => Self::ConditionPassed,
            mob_dsl::FlowRunReducerCommandKind::ConditionRejected => Self::ConditionRejected,
            mob_dsl::FlowRunReducerCommandKind::FailStep => Self::FailStep,
            mob_dsl::FlowRunReducerCommandKind::SkipStep => Self::SkipStep,
            mob_dsl::FlowRunReducerCommandKind::ProjectFrameStepStatus => {
                Self::ProjectFrameStepStatus
            }
            mob_dsl::FlowRunReducerCommandKind::CancelStep => Self::CancelStep,
            mob_dsl::FlowRunReducerCommandKind::RegisterTargets => Self::RegisterTargets,
            mob_dsl::FlowRunReducerCommandKind::RecordTargetSuccess => Self::RecordTargetSuccess,
            mob_dsl::FlowRunReducerCommandKind::RecordTargetTerminalFailure => {
                Self::RecordTargetTerminalFailure
            }
            mob_dsl::FlowRunReducerCommandKind::RecordTargetCanceled => Self::RecordTargetCanceled,
            mob_dsl::FlowRunReducerCommandKind::RecordTargetFailure => Self::RecordTargetFailure,
            mob_dsl::FlowRunReducerCommandKind::RegisterReadyFrame => Self::RegisterReadyFrame,
            mob_dsl::FlowRunReducerCommandKind::PumpNodeScheduler => Self::PumpNodeScheduler,
            mob_dsl::FlowRunReducerCommandKind::RegisterPendingBodyFrame => {
                Self::RegisterPendingBodyFrame
            }
            mob_dsl::FlowRunReducerCommandKind::PumpFrameScheduler => Self::PumpFrameScheduler,
            mob_dsl::FlowRunReducerCommandKind::NodeExecutionReleased => {
                Self::NodeExecutionReleased
            }
            mob_dsl::FlowRunReducerCommandKind::FrameTerminated => Self::FrameTerminated,
            mob_dsl::FlowRunReducerCommandKind::TerminalizeCompleted => Self::TerminalizeCompleted,
            mob_dsl::FlowRunReducerCommandKind::TerminalizeFailed => Self::TerminalizeFailed,
            mob_dsl::FlowRunReducerCommandKind::TerminalizeCanceled => Self::TerminalizeCanceled,
        }
    }
}

impl From<FlowRunReducerCommandRecord> for mob_dsl::FlowRunReducerCommandKind {
    fn from(kind: FlowRunReducerCommandRecord) -> Self {
        match kind {
            FlowRunReducerCommandRecord::CreateRun => Self::CreateRun,
            FlowRunReducerCommandRecord::StartRun => Self::StartRun,
            FlowRunReducerCommandRecord::DispatchStep => Self::DispatchStep,
            FlowRunReducerCommandRecord::CompleteStep => Self::CompleteStep,
            FlowRunReducerCommandRecord::RecordStepOutput => Self::RecordStepOutput,
            FlowRunReducerCommandRecord::ConditionPassed => Self::ConditionPassed,
            FlowRunReducerCommandRecord::ConditionRejected => Self::ConditionRejected,
            FlowRunReducerCommandRecord::FailStep => Self::FailStep,
            FlowRunReducerCommandRecord::SkipStep => Self::SkipStep,
            FlowRunReducerCommandRecord::ProjectFrameStepStatus => Self::ProjectFrameStepStatus,
            FlowRunReducerCommandRecord::CancelStep => Self::CancelStep,
            FlowRunReducerCommandRecord::RegisterTargets => Self::RegisterTargets,
            FlowRunReducerCommandRecord::RecordTargetSuccess => Self::RecordTargetSuccess,
            FlowRunReducerCommandRecord::RecordTargetTerminalFailure => {
                Self::RecordTargetTerminalFailure
            }
            FlowRunReducerCommandRecord::RecordTargetCanceled => Self::RecordTargetCanceled,
            FlowRunReducerCommandRecord::RecordTargetFailure => Self::RecordTargetFailure,
            FlowRunReducerCommandRecord::RegisterReadyFrame => Self::RegisterReadyFrame,
            FlowRunReducerCommandRecord::PumpNodeScheduler => Self::PumpNodeScheduler,
            FlowRunReducerCommandRecord::RegisterPendingBodyFrame => Self::RegisterPendingBodyFrame,
            FlowRunReducerCommandRecord::PumpFrameScheduler => Self::PumpFrameScheduler,
            FlowRunReducerCommandRecord::NodeExecutionReleased => Self::NodeExecutionReleased,
            FlowRunReducerCommandRecord::FrameTerminated => Self::FrameTerminated,
            FlowRunReducerCommandRecord::TerminalizeCompleted => Self::TerminalizeCompleted,
            FlowRunReducerCommandRecord::TerminalizeFailed => Self::TerminalizeFailed,
            FlowRunReducerCommandRecord::TerminalizeCanceled => Self::TerminalizeCanceled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowFrameReducerCommandRecord {
    StartRootFrame,
    StartBodyFrame,
    AdmitNextReadyNode,
    CompleteNode,
    RecordNodeOutput,
    FailNode,
    SkipNode,
    CancelNode,
    SealFrame,
}

impl From<mob_dsl::FlowFrameReducerCommandKind> for FlowFrameReducerCommandRecord {
    fn from(kind: mob_dsl::FlowFrameReducerCommandKind) -> Self {
        match kind {
            mob_dsl::FlowFrameReducerCommandKind::StartRootFrame => Self::StartRootFrame,
            mob_dsl::FlowFrameReducerCommandKind::StartBodyFrame => Self::StartBodyFrame,
            mob_dsl::FlowFrameReducerCommandKind::AdmitNextReadyNode => Self::AdmitNextReadyNode,
            mob_dsl::FlowFrameReducerCommandKind::CompleteNode => Self::CompleteNode,
            mob_dsl::FlowFrameReducerCommandKind::RecordNodeOutput => Self::RecordNodeOutput,
            mob_dsl::FlowFrameReducerCommandKind::FailNode => Self::FailNode,
            mob_dsl::FlowFrameReducerCommandKind::SkipNode => Self::SkipNode,
            mob_dsl::FlowFrameReducerCommandKind::CancelNode => Self::CancelNode,
            mob_dsl::FlowFrameReducerCommandKind::SealFrame => Self::SealFrame,
        }
    }
}

impl From<FlowFrameReducerCommandRecord> for mob_dsl::FlowFrameReducerCommandKind {
    fn from(kind: FlowFrameReducerCommandRecord) -> Self {
        match kind {
            FlowFrameReducerCommandRecord::StartRootFrame => Self::StartRootFrame,
            FlowFrameReducerCommandRecord::StartBodyFrame => Self::StartBodyFrame,
            FlowFrameReducerCommandRecord::AdmitNextReadyNode => Self::AdmitNextReadyNode,
            FlowFrameReducerCommandRecord::CompleteNode => Self::CompleteNode,
            FlowFrameReducerCommandRecord::RecordNodeOutput => Self::RecordNodeOutput,
            FlowFrameReducerCommandRecord::FailNode => Self::FailNode,
            FlowFrameReducerCommandRecord::SkipNode => Self::SkipNode,
            FlowFrameReducerCommandRecord::CancelNode => Self::CancelNode,
            FlowFrameReducerCommandRecord::SealFrame => Self::SealFrame,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopIterationReducerCommandRecord {
    StartLoop,
    BodyFrameStarted,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
    UntilConditionMet,
    UntilConditionFailed,
    CancelLoop,
}

impl From<mob_dsl::LoopIterationReducerCommandKind> for LoopIterationReducerCommandRecord {
    fn from(kind: mob_dsl::LoopIterationReducerCommandKind) -> Self {
        match kind {
            mob_dsl::LoopIterationReducerCommandKind::StartLoop => Self::StartLoop,
            mob_dsl::LoopIterationReducerCommandKind::BodyFrameStarted => Self::BodyFrameStarted,
            mob_dsl::LoopIterationReducerCommandKind::BodyFrameCompleted => {
                Self::BodyFrameCompleted
            }
            mob_dsl::LoopIterationReducerCommandKind::BodyFrameFailed => Self::BodyFrameFailed,
            mob_dsl::LoopIterationReducerCommandKind::BodyFrameCanceled => Self::BodyFrameCanceled,
            mob_dsl::LoopIterationReducerCommandKind::UntilConditionMet => Self::UntilConditionMet,
            mob_dsl::LoopIterationReducerCommandKind::UntilConditionFailed => {
                Self::UntilConditionFailed
            }
            mob_dsl::LoopIterationReducerCommandKind::CancelLoop => Self::CancelLoop,
        }
    }
}

impl From<LoopIterationReducerCommandRecord> for mob_dsl::LoopIterationReducerCommandKind {
    fn from(kind: LoopIterationReducerCommandRecord) -> Self {
        match kind {
            LoopIterationReducerCommandRecord::StartLoop => Self::StartLoop,
            LoopIterationReducerCommandRecord::BodyFrameStarted => Self::BodyFrameStarted,
            LoopIterationReducerCommandRecord::BodyFrameCompleted => Self::BodyFrameCompleted,
            LoopIterationReducerCommandRecord::BodyFrameFailed => Self::BodyFrameFailed,
            LoopIterationReducerCommandRecord::BodyFrameCanceled => Self::BodyFrameCanceled,
            LoopIterationReducerCommandRecord::UntilConditionMet => Self::UntilConditionMet,
            LoopIterationReducerCommandRecord::UntilConditionFailed => Self::UntilConditionFailed,
            LoopIterationReducerCommandRecord::CancelLoop => Self::CancelLoop,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRunSeedAuthorityRecord {
    pub run_id: mob_dsl::RunId,
    pub step_ids: BTreeSet<mob_dsl::StepId>,
    pub ordered_steps: Vec<mob_dsl::StepId>,
    pub step_has_conditions: BTreeMap<mob_dsl::StepId, bool>,
    pub step_dependencies: BTreeMap<mob_dsl::StepId, Vec<mob_dsl::StepId>>,
    pub step_dependency_modes: BTreeMap<mob_dsl::StepId, mob_dsl::DependencyMode>,
    pub step_branches: BTreeMap<mob_dsl::StepId, Option<mob_dsl::BranchId>>,
    pub step_collection_policies: BTreeMap<mob_dsl::StepId, mob_dsl::CollectionPolicyKind>,
    pub step_quorum_thresholds: BTreeMap<mob_dsl::StepId, u32>,
    pub escalation_threshold: u32,
    pub max_step_retries: u32,
    pub max_active_nodes: u64,
    pub max_active_frames: u64,
    pub max_frame_depth: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowFrameSeedAuthorityRecord {
    pub run_id: mob_dsl::RunId,
    pub frame_id: mob_dsl::FrameId,
    pub frame_scope: mob_dsl::FrameScope,
    pub loop_instance_id: Option<mob_dsl::LoopInstanceId>,
    pub iteration: u32,
    pub tracked_nodes: BTreeSet<mob_dsl::FlowNodeId>,
    pub ordered_nodes: Vec<mob_dsl::FlowNodeId>,
    pub node_kind: BTreeMap<mob_dsl::FlowNodeId, mob_dsl::FlowNodeKind>,
    pub node_dependencies: BTreeMap<mob_dsl::FlowNodeId, Vec<mob_dsl::FlowNodeId>>,
    pub node_dependency_modes: BTreeMap<mob_dsl::FlowNodeId, mob_dsl::DependencyMode>,
    pub node_branches: BTreeMap<mob_dsl::FlowNodeId, Option<mob_dsl::BranchId>>,
    pub node_step_ids: BTreeMap<mob_dsl::FlowNodeId, mob_dsl::StepId>,
    pub node_loop_ids: BTreeMap<mob_dsl::FlowNodeId, mob_dsl::LoopId>,
    pub node_status: BTreeMap<mob_dsl::FlowNodeId, mob_dsl::NodeRunStatus>,
    pub ready_queue: Vec<mob_dsl::FlowNodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "input", content = "payload")]
pub enum FlowAuthorityInputRecord {
    RunFlow(FlowRunSeedAuthorityRecord),
    CreateRunSeed(FlowRunSeedAuthorityRecord),
    AuthorizeFlowRunReducerCommand {
        run_id: mob_dsl::RunId,
        command: FlowRunReducerCommandRecord,
        step_id: Option<mob_dsl::StepId>,
        run_step_key: Option<mob_dsl::RunStepKey>,
        step_status: Option<mob_dsl::StepRunStatus>,
        target_count: Option<u64>,
        frame_id: Option<mob_dsl::FrameId>,
        node_id: Option<mob_dsl::FlowNodeId>,
        loop_instance_id: Option<mob_dsl::LoopInstanceId>,
        retry_key: Option<String>,
    },
    CreateFrameSeed(FlowFrameSeedAuthorityRecord),
    AuthorizeFlowFrameReducerCommand {
        frame_id: mob_dsl::FrameId,
        command: FlowFrameReducerCommandRecord,
        node_id: Option<mob_dsl::FlowNodeId>,
        frame_node_key: Option<mob_dsl::FrameNodeKey>,
        node_status: Option<mob_dsl::NodeRunStatus>,
        terminal_status: Option<mob_dsl::FrameStatus>,
    },
    CreateLoopSeed {
        loop_instance_id: mob_dsl::LoopInstanceId,
        parent_frame_id: mob_dsl::FrameId,
        parent_node_id: mob_dsl::FlowNodeId,
        loop_id: mob_dsl::LoopId,
        depth: u32,
        max_iterations: u64,
    },
    RecordLoopBodyFrameCompleted {
        loop_instance_id: mob_dsl::LoopInstanceId,
        iteration: u64,
    },
    RecordLoopUntilConditionMet {
        loop_instance_id: mob_dsl::LoopInstanceId,
        iteration: u64,
    },
    RecordLoopUntilConditionFailed {
        loop_instance_id: mob_dsl::LoopInstanceId,
        iteration: u64,
    },
    AuthorizeLoopIterationReducerCommand {
        loop_instance_id: mob_dsl::LoopInstanceId,
        command: LoopIterationReducerCommandRecord,
        body_frame_id: Option<mob_dsl::FrameId>,
        body_frame_iteration: Option<u64>,
    },
}

impl FlowAuthorityInputRecord {
    pub(crate) fn from_machine_input(input: mob_dsl::MobMachineInput) -> Result<Self, MobError> {
        let record = match input {
            mob_dsl::MobMachineInput::RunFlow {
                run_id,
                step_ids,
                ordered_steps,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                escalation_threshold,
                max_step_retries,
                max_active_nodes,
                max_active_frames,
                max_frame_depth,
            } => Self::RunFlow(FlowRunSeedAuthorityRecord {
                run_id,
                step_ids,
                ordered_steps,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                escalation_threshold,
                max_step_retries,
                max_active_nodes,
                max_active_frames,
                max_frame_depth,
            }),
            mob_dsl::MobMachineInput::CreateRunSeed {
                run_id,
                step_ids,
                ordered_steps,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                escalation_threshold,
                max_step_retries,
                max_active_nodes,
                max_active_frames,
                max_frame_depth,
            } => Self::CreateRunSeed(FlowRunSeedAuthorityRecord {
                run_id,
                step_ids,
                ordered_steps,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                escalation_threshold,
                max_step_retries,
                max_active_nodes,
                max_active_frames,
                max_frame_depth,
            }),
            mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand {
                run_id,
                command,
                step_id,
                run_step_key,
                step_status,
                target_count,
                frame_id,
                node_id,
                loop_instance_id,
                retry_key,
            } => Self::AuthorizeFlowRunReducerCommand {
                run_id,
                command: command.into(),
                step_id,
                run_step_key,
                step_status,
                target_count,
                frame_id,
                node_id,
                loop_instance_id,
                retry_key,
            },
            mob_dsl::MobMachineInput::CreateFrameSeed {
                run_id,
                frame_id,
                frame_scope,
                loop_instance_id,
                iteration,
                tracked_nodes,
                ordered_nodes,
                node_kind,
                node_dependencies,
                node_dependency_modes,
                node_branches,
                node_step_ids,
                node_loop_ids,
                node_status,
                ready_queue,
            } => Self::CreateFrameSeed(FlowFrameSeedAuthorityRecord {
                run_id,
                frame_id,
                frame_scope,
                loop_instance_id,
                iteration,
                tracked_nodes,
                ordered_nodes,
                node_kind,
                node_dependencies,
                node_dependency_modes,
                node_branches,
                node_step_ids,
                node_loop_ids,
                node_status,
                ready_queue,
            }),
            mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command,
                node_id,
                frame_node_key,
                node_status,
                terminal_status,
            } => Self::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command: command.into(),
                node_id,
                frame_node_key,
                node_status,
                terminal_status,
            },
            mob_dsl::MobMachineInput::CreateLoopSeed {
                loop_instance_id,
                parent_frame_id,
                parent_node_id,
                loop_id,
                depth,
                max_iterations,
            } => Self::CreateLoopSeed {
                loop_instance_id,
                parent_frame_id,
                parent_node_id,
                loop_id,
                depth,
                max_iterations,
            },
            mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted {
                loop_instance_id,
                iteration,
            } => Self::RecordLoopBodyFrameCompleted {
                loop_instance_id,
                iteration,
            },
            mob_dsl::MobMachineInput::RecordLoopUntilConditionMet {
                loop_instance_id,
                iteration,
            } => Self::RecordLoopUntilConditionMet {
                loop_instance_id,
                iteration,
            },
            mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed {
                loop_instance_id,
                iteration,
            } => Self::RecordLoopUntilConditionFailed {
                loop_instance_id,
                iteration,
            },
            mob_dsl::MobMachineInput::AuthorizeLoopIterationReducerCommand {
                loop_instance_id,
                command,
                body_frame_id,
                body_frame_iteration,
            } => Self::AuthorizeLoopIterationReducerCommand {
                loop_instance_id,
                command: command.into(),
                body_frame_id,
                body_frame_iteration,
            },
            mob_dsl::MobMachineInput::CancelFlow { .. }
            | mob_dsl::MobMachineInput::FlowStatus
            | mob_dsl::MobMachineInput::Spawn { .. }
            | mob_dsl::MobMachineInput::EnsureMember { .. }
            | mob_dsl::MobMachineInput::Reconcile { .. }
            | mob_dsl::MobMachineInput::Retire { .. }
            | mob_dsl::MobMachineInput::Respawn { .. }
            | mob_dsl::MobMachineInput::RetireAll
            | mob_dsl::MobMachineInput::WireMembers { .. }
            | mob_dsl::MobMachineInput::UnwireMembers { .. }
            | mob_dsl::MobMachineInput::WireExternalPeer { .. }
            | mob_dsl::MobMachineInput::UnwireExternalPeer { .. }
            | mob_dsl::MobMachineInput::SubmitWork { .. }
            | mob_dsl::MobMachineInput::CancelWork { .. }
            | mob_dsl::MobMachineInput::CancelAllWork { .. }
            | mob_dsl::MobMachineInput::Stop
            | mob_dsl::MobMachineInput::Resume
            | mob_dsl::MobMachineInput::Complete
            | mob_dsl::MobMachineInput::Reset
            | mob_dsl::MobMachineInput::Destroy
            | mob_dsl::MobMachineInput::TaskCreate { .. }
            | mob_dsl::MobMachineInput::TaskUpdate { .. }
            | mob_dsl::MobMachineInput::TaskList
            | mob_dsl::MobMachineInput::TaskGet
            | mob_dsl::MobMachineInput::RosterSnapshot
            | mob_dsl::MobMachineInput::ListMembers
            | mob_dsl::MobMachineInput::ListMembersIncludingRetiring
            | mob_dsl::MobMachineInput::ListAllMembers
            | mob_dsl::MobMachineInput::MemberStatus
            | mob_dsl::MobMachineInput::SubscribeAgentEvents
            | mob_dsl::MobMachineInput::SubscribeAllAgentEvents
            | mob_dsl::MobMachineInput::SubscribeMobEvents
            | mob_dsl::MobMachineInput::PollEvents
            | mob_dsl::MobMachineInput::ReplayAllEvents
            | mob_dsl::MobMachineInput::RecordOperatorActionProvenance
            | mob_dsl::MobMachineInput::GetMember
            | mob_dsl::MobMachineInput::SetSpawnPolicy
            | mob_dsl::MobMachineInput::Shutdown
            | mob_dsl::MobMachineInput::ForceCancel
            | mob_dsl::MobMachineInput::KickoffMarkPending { .. }
            | mob_dsl::MobMachineInput::KickoffMarkStarting { .. }
            | mob_dsl::MobMachineInput::StartupMarkReady { .. }
            | mob_dsl::MobMachineInput::KickoffResolveStarted { .. }
            | mob_dsl::MobMachineInput::KickoffResolveCallbackPending { .. }
            | mob_dsl::MobMachineInput::KickoffResolveFailed { .. }
            | mob_dsl::MobMachineInput::KickoffCancelRequested { .. }
            | mob_dsl::MobMachineInput::KickoffClear { .. } => {
                return Err(MobError::Internal(format!(
                    "MobMachine input {input:?} is not a flow authority input"
                )));
            }
            crate::mob_destroying_session_ingress_feedback_input_patterns!() => {
                return Err(MobError::Internal(format!(
                    "MobMachine input {input:?} is not a flow authority input"
                )));
            }
        };
        let input_variant = record.input_variant();
        if !canonical_flow_authority_input_variant_manifest().contains(&input_variant) {
            return Err(MobError::Internal(format!(
                "MobMachine input variant {input_variant:?} is not in the typed flow authority manifest"
            )));
        }
        Ok(record)
    }

    #[must_use]
    pub fn input_variant(&self) -> MobMachineInputVariant {
        self.catalog_input().input_variant()
    }

    #[must_use]
    fn catalog_input(&self) -> MobMachineCatalogInput {
        match self {
            Self::RunFlow(_) => MobMachineCatalogInput::RunFlow,
            Self::CreateRunSeed(_) => MobMachineCatalogInput::CreateRunSeed,
            Self::AuthorizeFlowRunReducerCommand { .. } => {
                MobMachineCatalogInput::AuthorizeFlowRunReducerCommand
            }
            Self::CreateFrameSeed(_) => MobMachineCatalogInput::CreateFrameSeed,
            Self::AuthorizeFlowFrameReducerCommand { .. } => {
                MobMachineCatalogInput::AuthorizeFlowFrameReducerCommand
            }
            Self::CreateLoopSeed { .. } => MobMachineCatalogInput::CreateLoopSeed,
            Self::RecordLoopBodyFrameCompleted { .. } => {
                MobMachineCatalogInput::RecordLoopBodyFrameCompleted
            }
            Self::RecordLoopUntilConditionMet { .. } => {
                MobMachineCatalogInput::RecordLoopUntilConditionMet
            }
            Self::RecordLoopUntilConditionFailed { .. } => {
                MobMachineCatalogInput::RecordLoopUntilConditionFailed
            }
            Self::AuthorizeLoopIterationReducerCommand { .. } => {
                MobMachineCatalogInput::AuthorizeLoopIterationReducerCommand
            }
        }
    }

    pub(crate) fn to_machine_input(&self) -> mob_dsl::MobMachineInput {
        match self.clone() {
            Self::RunFlow(record) => mob_dsl::MobMachineInput::RunFlow {
                run_id: record.run_id,
                step_ids: record.step_ids,
                ordered_steps: record.ordered_steps,
                step_has_conditions: record.step_has_conditions,
                step_dependencies: record.step_dependencies,
                step_dependency_modes: record.step_dependency_modes,
                step_branches: record.step_branches,
                step_collection_policies: record.step_collection_policies,
                step_quorum_thresholds: record.step_quorum_thresholds,
                escalation_threshold: record.escalation_threshold,
                max_step_retries: record.max_step_retries,
                max_active_nodes: record.max_active_nodes,
                max_active_frames: record.max_active_frames,
                max_frame_depth: record.max_frame_depth,
            },
            Self::CreateRunSeed(record) => mob_dsl::MobMachineInput::CreateRunSeed {
                run_id: record.run_id,
                step_ids: record.step_ids,
                ordered_steps: record.ordered_steps,
                step_has_conditions: record.step_has_conditions,
                step_dependencies: record.step_dependencies,
                step_dependency_modes: record.step_dependency_modes,
                step_branches: record.step_branches,
                step_collection_policies: record.step_collection_policies,
                step_quorum_thresholds: record.step_quorum_thresholds,
                escalation_threshold: record.escalation_threshold,
                max_step_retries: record.max_step_retries,
                max_active_nodes: record.max_active_nodes,
                max_active_frames: record.max_active_frames,
                max_frame_depth: record.max_frame_depth,
            },
            Self::AuthorizeFlowRunReducerCommand {
                run_id,
                command,
                step_id,
                run_step_key,
                step_status,
                target_count,
                frame_id,
                node_id,
                loop_instance_id,
                retry_key,
            } => mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand {
                run_id,
                command: command.into(),
                step_id,
                run_step_key,
                step_status,
                target_count,
                frame_id,
                node_id,
                loop_instance_id,
                retry_key,
            },
            Self::CreateFrameSeed(record) => mob_dsl::MobMachineInput::CreateFrameSeed {
                run_id: record.run_id,
                frame_id: record.frame_id,
                frame_scope: record.frame_scope,
                loop_instance_id: record.loop_instance_id,
                iteration: record.iteration,
                tracked_nodes: record.tracked_nodes,
                ordered_nodes: record.ordered_nodes,
                node_kind: record.node_kind,
                node_dependencies: record.node_dependencies,
                node_dependency_modes: record.node_dependency_modes,
                node_branches: record.node_branches,
                node_step_ids: record.node_step_ids,
                node_loop_ids: record.node_loop_ids,
                node_status: record.node_status,
                ready_queue: record.ready_queue,
            },
            Self::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command,
                node_id,
                frame_node_key,
                node_status,
                terminal_status,
            } => mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command: command.into(),
                node_id,
                frame_node_key,
                node_status,
                terminal_status,
            },
            Self::CreateLoopSeed {
                loop_instance_id,
                parent_frame_id,
                parent_node_id,
                loop_id,
                depth,
                max_iterations,
            } => mob_dsl::MobMachineInput::CreateLoopSeed {
                loop_instance_id,
                parent_frame_id,
                parent_node_id,
                loop_id,
                depth,
                max_iterations,
            },
            Self::RecordLoopBodyFrameCompleted {
                loop_instance_id,
                iteration,
            } => mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted {
                loop_instance_id,
                iteration,
            },
            Self::RecordLoopUntilConditionMet {
                loop_instance_id,
                iteration,
            } => mob_dsl::MobMachineInput::RecordLoopUntilConditionMet {
                loop_instance_id,
                iteration,
            },
            Self::RecordLoopUntilConditionFailed {
                loop_instance_id,
                iteration,
            } => mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed {
                loop_instance_id,
                iteration,
            },
            Self::AuthorizeLoopIterationReducerCommand {
                loop_instance_id,
                command,
                body_frame_id,
                body_frame_iteration,
            } => mob_dsl::MobMachineInput::AuthorizeLoopIterationReducerCommand {
                loop_instance_id,
                command: command.into(),
                body_frame_id,
                body_frame_iteration,
            },
        }
    }
}

pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    match command {
        MobMachineFlowRunCommand::CreateRun(_) => {
            project_flow_run_state_from_machine(machine_state, run_id).map(|next_state| {
                flow_run::Outcome {
                    transition_id: flow_run::TransitionId::CreateRun,
                    next_state,
                    effects: vec![flow_run::Effect::EmitFlowRunNotice(
                        flow_run::effects::EmitFlowRunNotice {
                            run_status: flow_run::FlowRunStatus::Pending,
                        },
                    )],
                }
            })
        }
        MobMachineFlowRunCommand::StartRun(_) => {
            let next_state = project_flow_run_state_from_machine(machine_state, run_id)?;
            if next_state.phase != flow_run::Phase::Running {
                return Err(MobError::Internal(format!(
                    "MobMachine run '{run_id}' projected phase {:?}, expected {:?}",
                    next_state.phase,
                    flow_run::Phase::Running
                )));
            }
            Ok(flow_run::Outcome {
                transition_id: flow_run::TransitionId::StartRun,
                next_state,
                effects: vec![flow_run::Effect::EmitFlowRunNotice(
                    flow_run::effects::EmitFlowRunNotice {
                        run_status: flow_run::FlowRunStatus::Running,
                    },
                )],
            })
        }
        MobMachineFlowRunCommand::DispatchStep(payload) => {
            project_flow_run_step_status_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                flow_run::StepRunStatus::Dispatched,
                flow_run::TransitionId::DispatchStep,
            )
        }
        MobMachineFlowRunCommand::CompleteStep(payload) => {
            project_flow_run_completed_step_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
            )
        }
        MobMachineFlowRunCommand::RecordStepOutput(payload) => {
            project_flow_run_step_output_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
            )
        }
        MobMachineFlowRunCommand::ConditionPassed(payload) => {
            project_flow_run_condition_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                true,
                flow_run::TransitionId::ConditionPassed,
            )
        }
        MobMachineFlowRunCommand::ConditionRejected(payload) => {
            project_flow_run_condition_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                false,
                flow_run::TransitionId::ConditionRejected,
            )
        }
        MobMachineFlowRunCommand::FailStep(payload) => project_flow_run_failed_step_from_machine(
            state,
            machine_state,
            run_id,
            &payload.step_id,
        ),
        MobMachineFlowRunCommand::SkipStep(payload) => project_flow_run_step_status_from_machine(
            state,
            machine_state,
            run_id,
            &payload.step_id,
            flow_run::StepRunStatus::Skipped,
            flow_run::TransitionId::SkipStep,
        ),
        MobMachineFlowRunCommand::ProjectFrameStepStatus(payload) => {
            project_flow_run_frame_step_status_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                &payload.frame_id,
                &payload.node_id,
                payload.append_failure_ledger,
            )
        }
        MobMachineFlowRunCommand::CancelStep(payload) => project_flow_run_step_status_from_machine(
            state,
            machine_state,
            run_id,
            &payload.step_id,
            flow_run::StepRunStatus::Canceled,
            flow_run::TransitionId::CancelStep,
        ),
        MobMachineFlowRunCommand::RegisterTargets(payload) => {
            project_flow_run_target_count_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                payload.target_count,
            )
        }
        MobMachineFlowRunCommand::RecordTargetSuccess(payload) => {
            project_flow_run_target_success_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                &payload.target_id,
            )
        }
        MobMachineFlowRunCommand::RecordTargetTerminalFailure(payload) => {
            project_flow_run_target_terminal_failure_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
            )
        }
        MobMachineFlowRunCommand::RecordTargetCanceled(payload) => {
            project_flow_run_target_canceled_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                &payload.target_id,
            )
        }
        MobMachineFlowRunCommand::RecordTargetFailure(payload) => {
            project_flow_run_target_failure_from_machine(
                state,
                machine_state,
                run_id,
                &payload.step_id,
                &payload.target_id,
                &payload.retry_key,
            )
        }
        MobMachineFlowRunCommand::RegisterReadyFrame(payload) => {
            project_flow_run_ready_frames_from_machine(
                state,
                machine_state,
                run_id,
                flow_run::TransitionId::RegisterReadyFrame,
                None,
                Some(&payload.frame_id),
            )
        }
        MobMachineFlowRunCommand::PumpNodeScheduler(_) => {
            project_flow_run_node_scheduler_from_machine(state, machine_state, run_id)
        }
        MobMachineFlowRunCommand::RegisterPendingBodyFrame(payload) => {
            project_flow_run_pending_body_frames_from_machine(
                state,
                machine_state,
                run_id,
                flow_run::TransitionId::RegisterPendingBodyFrame,
                None,
                Some(&payload.loop_instance_id),
            )
        }
        MobMachineFlowRunCommand::PumpFrameScheduler(_) => {
            project_flow_run_frame_scheduler_from_machine(state, machine_state, run_id)
        }
        MobMachineFlowRunCommand::NodeExecutionReleased(payload) => {
            project_flow_run_ready_frames_from_machine(
                state,
                machine_state,
                run_id,
                flow_run::TransitionId::NodeExecutionReleased,
                None,
                Some(&payload.frame_id),
            )
        }
        MobMachineFlowRunCommand::FrameTerminated(payload) => {
            let _ = payload.frame_id;
            project_flow_run_pending_body_frames_from_machine(
                state,
                machine_state,
                run_id,
                flow_run::TransitionId::FrameTerminated,
                None,
                None,
            )
        }
        MobMachineFlowRunCommand::TerminalizeCompleted(_) => {
            project_flow_run_terminal_from_machine(
                state,
                machine_state,
                run_id,
                flow_run::Phase::Completed,
                flow_run::FlowRunStatus::Completed,
                flow_run::TransitionId::TerminalizeCompleted,
            )
        }
        MobMachineFlowRunCommand::TerminalizeFailed(_) => project_flow_run_terminal_from_machine(
            state,
            machine_state,
            run_id,
            flow_run::Phase::Failed,
            flow_run::FlowRunStatus::Failed,
            flow_run::TransitionId::TerminalizeFailed,
        ),
        MobMachineFlowRunCommand::TerminalizeCanceled(_) => project_flow_run_terminal_from_machine(
            state,
            machine_state,
            run_id,
            flow_run::Phase::Canceled,
            flow_run::FlowRunStatus::Canceled,
            flow_run::TransitionId::TerminalizeCanceled,
        ),
    }
}

pub(crate) fn apply_mob_machine_flow_frame_command(
    state: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
    command: MobMachineFlowFrameCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_frame::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowFrame(command.kind()))?;
    match command {
        MobMachineFlowFrameCommand::StartRootFrame(payload) => {
            project_flow_frame_seed_from_machine(
                machine_state,
                &payload.frame_id,
                flow_frame::TransitionId::StartRootFrame,
            )
        }
        MobMachineFlowFrameCommand::StartBodyFrame(payload) => {
            project_flow_frame_seed_from_machine(
                machine_state,
                &payload.frame_id,
                flow_frame::TransitionId::StartBodyFrame,
            )
        }
        MobMachineFlowFrameCommand::AdmitNextReadyNode(_) => {
            project_flow_frame_admit_from_machine(state, machine_state)
        }
        MobMachineFlowFrameCommand::CompleteNode(payload) => project_flow_frame_node_from_machine(
            state,
            machine_state,
            &payload.node_id,
            flow_frame::NodeRunStatus::Completed,
            flow_frame::TransitionId::CompleteNode,
        ),
        MobMachineFlowFrameCommand::RecordNodeOutput(payload) => {
            project_flow_frame_node_output_from_machine(state, machine_state, &payload.node_id)
        }
        MobMachineFlowFrameCommand::FailNode(payload) => project_flow_frame_node_from_machine(
            state,
            machine_state,
            &payload.node_id,
            flow_frame::NodeRunStatus::Failed,
            flow_frame::TransitionId::FailNode,
        ),
        MobMachineFlowFrameCommand::SkipNode(payload) => project_flow_frame_node_from_machine(
            state,
            machine_state,
            &payload.node_id,
            flow_frame::NodeRunStatus::Skipped,
            flow_frame::TransitionId::SkipNode,
        ),
        MobMachineFlowFrameCommand::CancelNode(payload) => project_flow_frame_node_from_machine(
            state,
            machine_state,
            &payload.node_id,
            flow_frame::NodeRunStatus::Canceled,
            flow_frame::TransitionId::CancelNode,
        ),
        MobMachineFlowFrameCommand::SealFrame(payload) => {
            project_flow_frame_seal_from_machine(state, machine_state, payload.terminal_status)
        }
    }
}

pub(crate) fn apply_mob_machine_loop_iteration_command(
    _state: &loop_iteration::State,
    machine_state: &mob_dsl::MobMachineState,
    command: MobMachineLoopIterationCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<loop_iteration::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::LoopIteration(command.kind()))?;
    match command {
        MobMachineLoopIterationCommand::StartLoop(payload) => project_loop_iteration_from_machine(
            machine_state,
            &payload.loop_instance_id,
            loop_iteration::TransitionId::StartLoop,
            vec![loop_iteration::Effect::RequestBodyFrameStart(
                loop_iteration::effects::RequestBodyFrameStart {
                    loop_instance_id: payload.loop_instance_id.clone(),
                    depth: payload.depth,
                },
            )],
        ),
        MobMachineLoopIterationCommand::BodyFrameStarted(payload) => {
            project_loop_iteration_from_machine(
                machine_state,
                &payload.loop_instance_id,
                loop_iteration::TransitionId::BodyFrameStarted,
                Vec::new(),
            )
        }
        MobMachineLoopIterationCommand::BodyFrameCompleted(payload) => {
            let projected = project_loop_iteration_state_from_machine(
                machine_state,
                &payload.loop_instance_id,
            )?;
            let effects = vec![loop_iteration::Effect::EvaluateUntilCondition(
                loop_iteration::effects::EvaluateUntilCondition {
                    loop_instance_id: payload.loop_instance_id,
                    iteration: payload.iteration,
                    parent_frame_id: projected.parent_frame_id.clone(),
                    parent_node_id: projected.parent_node_id.clone(),
                    loop_id: projected.loop_id.clone(),
                },
            )];
            Ok(loop_iteration::Outcome {
                transition_id: loop_iteration::TransitionId::BodyFrameCompleted,
                next_state: projected,
                effects,
            })
        }
        MobMachineLoopIterationCommand::UntilConditionMet(payload) => {
            let projected = project_loop_iteration_state_from_machine(
                machine_state,
                &payload.loop_instance_id,
            )?;
            let effects = vec![loop_iteration::Effect::LoopCompleted(
                loop_iteration::effects::LoopCompleted {
                    loop_instance_id: payload.loop_instance_id,
                    parent_frame_id: projected.parent_frame_id.clone(),
                    parent_node_id: projected.parent_node_id.clone(),
                },
            )];
            Ok(loop_iteration::Outcome {
                transition_id: loop_iteration::TransitionId::UntilConditionMet,
                next_state: projected,
                effects,
            })
        }
        MobMachineLoopIterationCommand::UntilConditionFailed(payload) => {
            let projected = project_loop_iteration_state_from_machine(
                machine_state,
                &payload.loop_instance_id,
            )?;
            let effects = match projected.phase {
                loop_iteration::Phase::Exhausted => vec![loop_iteration::Effect::LoopExhausted(
                    loop_iteration::effects::LoopExhausted {
                        loop_instance_id: payload.loop_instance_id,
                        parent_frame_id: projected.parent_frame_id.clone(),
                        parent_node_id: projected.parent_node_id.clone(),
                    },
                )],
                loop_iteration::Phase::Running => {
                    vec![loop_iteration::Effect::RequestBodyFrameStart(
                        loop_iteration::effects::RequestBodyFrameStart {
                            loop_instance_id: payload.loop_instance_id,
                            depth: projected.depth,
                        },
                    )]
                }
                _ => Vec::new(),
            };
            Ok(loop_iteration::Outcome {
                transition_id: loop_iteration::TransitionId::UntilConditionFailed,
                next_state: projected,
                effects,
            })
        }
        MobMachineLoopIterationCommand::BodyFrameFailed(payload) => {
            let projected = project_loop_iteration_state_from_machine(
                machine_state,
                &payload.loop_instance_id,
            )?;
            Ok(loop_iteration::Outcome {
                transition_id: loop_iteration::TransitionId::BodyFrameFailed,
                effects: vec![loop_iteration::Effect::LoopFailed(
                    loop_iteration::effects::LoopFailed {
                        loop_instance_id: payload.loop_instance_id,
                        parent_frame_id: projected.parent_frame_id.clone(),
                        parent_node_id: projected.parent_node_id.clone(),
                    },
                )],
                next_state: projected,
            })
        }
        MobMachineLoopIterationCommand::BodyFrameCanceled(payload) => {
            let projected = project_loop_iteration_state_from_machine(
                machine_state,
                &payload.loop_instance_id,
            )?;
            Ok(loop_iteration::Outcome {
                transition_id: loop_iteration::TransitionId::BodyFrameCanceled,
                effects: vec![loop_iteration::Effect::LoopCanceled(
                    loop_iteration::effects::LoopCanceled {
                        loop_instance_id: payload.loop_instance_id,
                        parent_frame_id: projected.parent_frame_id.clone(),
                        parent_node_id: projected.parent_node_id.clone(),
                    },
                )],
                next_state: projected,
            })
        }
        MobMachineLoopIterationCommand::CancelLoop(payload) => {
            let projected = project_loop_iteration_state_from_machine(
                machine_state,
                &payload.loop_instance_id,
            )?;
            Ok(loop_iteration::Outcome {
                transition_id: loop_iteration::TransitionId::CancelLoop,
                effects: vec![loop_iteration::Effect::LoopCanceled(
                    loop_iteration::effects::LoopCanceled {
                        loop_instance_id: payload.loop_instance_id,
                        parent_frame_id: projected.parent_frame_id.clone(),
                        parent_node_id: projected.parent_node_id.clone(),
                    },
                )],
                next_state: projected,
            })
        }
    }
}

pub(crate) fn project_flow_run_state_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
) -> Result<flow_run::State, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let phase = match required_machine_value(&machine_state.run_status, &run_key, "run_status")? {
        mob_dsl::FlowRunStatus::Absent => flow_run::Phase::Absent,
        mob_dsl::FlowRunStatus::Pending => flow_run::Phase::Pending,
        mob_dsl::FlowRunStatus::Running => flow_run::Phase::Running,
        mob_dsl::FlowRunStatus::Completed => flow_run::Phase::Completed,
        mob_dsl::FlowRunStatus::Failed => flow_run::Phase::Failed,
        mob_dsl::FlowRunStatus::Canceled => flow_run::Phase::Canceled,
    };
    let tracked_steps = required_machine_value(
        &machine_state.run_tracked_steps,
        &run_key,
        "run_tracked_steps",
    )?
    .iter()
    .map(project_step_id)
    .collect::<BTreeSet<_>>();
    let ordered_steps = required_machine_value(
        &machine_state.run_ordered_steps,
        &run_key,
        "run_ordered_steps",
    )?
    .iter()
    .map(project_step_id)
    .collect::<Vec<_>>();

    let mut state = flow_run::initial_state();
    state.phase = phase;
    state.tracked_steps = tracked_steps;
    state.ordered_steps = ordered_steps;
    state.step_status = project_step_option_status_map(
        machine_state
            .run_step_status
            .get(&run_key)
            .cloned()
            .unwrap_or_default(),
    );
    for (key, status) in &machine_state.run_step_status_flat {
        if let Some(step_id) = project_run_step_key_for_run(key, run_id) {
            state
                .step_status
                .insert(step_id, Some(project_step_run_status(*status)));
        }
    }
    state.output_recorded = project_step_map(
        machine_state
            .run_output_recorded
            .get(&run_key)
            .cloned()
            .unwrap_or_default(),
        |v| v,
    );
    for (key, recorded) in &machine_state.run_output_recorded_flat {
        if let Some(step_id) = project_run_step_key_for_run(key, run_id) {
            state.output_recorded.insert(step_id, *recorded);
        }
    }
    state.step_condition_results = project_step_map(
        machine_state
            .run_step_condition_results
            .get(&run_key)
            .cloned()
            .unwrap_or_default(),
        |v| v,
    );
    for (key, result) in &machine_state.run_step_condition_results_flat {
        if let Some(step_id) = project_run_step_key_for_run(key, run_id) {
            state.step_condition_results.insert(step_id, *result);
        }
    }
    state.step_has_conditions = project_step_map(
        required_machine_value(
            &machine_state.run_step_has_conditions,
            &run_key,
            "run_step_has_conditions",
        )?
        .clone(),
        |v| v,
    );
    state.step_dependencies = project_step_map(
        required_machine_value(
            &machine_state.run_step_dependencies,
            &run_key,
            "run_step_dependencies",
        )?
        .clone(),
        |deps| deps.iter().map(project_step_id).collect(),
    );
    state.step_dependency_modes = project_step_map(
        required_machine_value(
            &machine_state.run_step_dependency_modes,
            &run_key,
            "run_step_dependency_modes",
        )?
        .clone(),
        project_flow_run_dependency_mode,
    );
    state.step_branches = project_step_map(
        required_machine_value(
            &machine_state.run_step_branches,
            &run_key,
            "run_step_branches",
        )?
        .clone(),
        |branch| branch.as_ref().map(project_branch_id),
    );
    state.step_collection_policies = project_step_map(
        required_machine_value(
            &machine_state.run_step_collection_policies,
            &run_key,
            "run_step_collection_policies",
        )?
        .clone(),
        project_collection_policy,
    );
    state.step_quorum_thresholds = project_step_map(
        required_machine_value(
            &machine_state.run_step_quorum_thresholds,
            &run_key,
            "run_step_quorum_thresholds",
        )?
        .clone(),
        |v| v,
    );
    state.step_target_counts = project_step_map(
        machine_state
            .run_step_target_counts
            .get(&run_key)
            .cloned()
            .unwrap_or_default(),
        saturating_u64_to_u32,
    );
    for (key, count) in &machine_state.run_step_target_counts_flat {
        if let Some(step_id) = project_run_step_key_for_run(key, run_id) {
            state
                .step_target_counts
                .insert(step_id, saturating_u64_to_u32(*count));
        }
    }
    state.step_target_success_counts = project_step_map(
        machine_state
            .run_step_target_success_counts
            .get(&run_key)
            .cloned()
            .unwrap_or_default(),
        saturating_u64_to_u32,
    );
    for (key, count) in &machine_state.run_step_target_success_counts_flat {
        if let Some(step_id) = project_run_step_key_for_run(key, run_id) {
            state
                .step_target_success_counts
                .insert(step_id, saturating_u64_to_u32(*count));
        }
    }
    state.step_target_terminal_failure_counts = project_step_map(
        machine_state
            .run_step_target_terminal_failure_counts
            .get(&run_key)
            .cloned()
            .unwrap_or_default(),
        saturating_u64_to_u32,
    );
    for (key, count) in &machine_state.run_step_target_terminal_failure_counts_flat {
        if let Some(step_id) = project_run_step_key_for_run(key, run_id) {
            state
                .step_target_terminal_failure_counts
                .insert(step_id, saturating_u64_to_u32(*count));
        }
    }
    state.target_retry_counts = machine_state
        .run_target_retry_counts
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, saturating_u64_to_u32(value)))
        .collect();
    project_flow_run_counters_from_machine(&mut state, machine_state, &run_key)?;
    state.escalation_threshold = *required_machine_value(
        &machine_state.run_escalation_threshold,
        &run_key,
        "run_escalation_threshold",
    )?;
    state.max_step_retries = *required_machine_value(
        &machine_state.run_max_step_retries,
        &run_key,
        "run_max_step_retries",
    )?;
    state.ready_frames = machine_state
        .run_ready_frames
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_frame_id)
        .collect();
    state.ready_frame_membership = machine_state
        .run_ready_frame_membership
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_frame_id)
        .collect();
    for frame_id in &machine_state.run_ready_frame_membership_flat {
        if machine_state.frame_run.get(frame_id) == Some(&run_key) {
            state
                .ready_frame_membership
                .insert(project_frame_id(frame_id));
        }
    }
    if !state.ready_frame_membership.is_empty() {
        state.ready_frames = state.ready_frame_membership.iter().cloned().collect();
    }
    state.pending_body_frame_loops = machine_state
        .run_pending_body_frame_loops
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_loop_instance_id)
        .collect();
    let mut pending_body_frame_loop_membership: BTreeSet<LoopInstanceId> = machine_state
        .run_pending_body_frame_loop_membership
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_loop_instance_id)
        .collect();
    for loop_instance_id in &machine_state.run_pending_body_frame_loop_membership_flat {
        if machine_state
            .loop_parent_frame
            .get(loop_instance_id)
            .and_then(|frame_id| machine_state.frame_run.get(frame_id))
            == Some(&run_key)
        {
            pending_body_frame_loop_membership.insert(project_loop_instance_id(loop_instance_id));
        }
    }
    state.pending_body_frame_loop_membership = pending_body_frame_loop_membership;
    if !state.pending_body_frame_loop_membership.is_empty() {
        state.pending_body_frame_loops = state
            .pending_body_frame_loop_membership
            .iter()
            .cloned()
            .collect();
    }
    state.active_node_count = required_machine_value(
        &machine_state.run_active_node_count,
        &run_key,
        "run_active_node_count",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    state.active_frame_count = required_machine_value(
        &machine_state.run_active_frame_count,
        &run_key,
        "run_active_frame_count",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    if let Some(frame_id) = machine_state.run_last_granted_frame.get(&run_key) {
        state.last_granted_frame = project_frame_id(frame_id);
    }
    if let Some(loop_instance_id) = machine_state.run_last_granted_loop.get(&run_key) {
        state.last_granted_loop = project_loop_instance_id(loop_instance_id);
    }
    state.max_active_nodes = required_machine_value(
        &machine_state.run_max_active_nodes,
        &run_key,
        "run_max_active_nodes",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    state.max_active_frames = required_machine_value(
        &machine_state.run_max_active_frames,
        &run_key,
        "run_max_active_frames",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    state.max_frame_depth = required_machine_value(
        &machine_state.run_max_frame_depth,
        &run_key,
        "run_max_frame_depth",
    )
    .map(|value| saturating_u64_to_u32(*value))?;

    for step_id in state.tracked_steps.clone() {
        state.step_status.entry(step_id.clone()).or_insert(None);
        state
            .output_recorded
            .entry(step_id.clone())
            .or_insert(false);
        state
            .step_condition_results
            .entry(step_id.clone())
            .or_insert(None);
        state.step_dependencies.entry(step_id.clone()).or_default();
        state
            .step_dependency_modes
            .entry(step_id.clone())
            .or_insert(flow_run::DependencyMode::All);
        state.step_branches.entry(step_id.clone()).or_insert(None);
        state
            .step_collection_policies
            .entry(step_id.clone())
            .or_insert(flow_run::CollectionPolicyKind::All);
        state
            .step_quorum_thresholds
            .entry(step_id.clone())
            .or_insert(0);
        state.step_target_counts.entry(step_id.clone()).or_insert(0);
        state
            .step_target_success_counts
            .entry(step_id.clone())
            .or_insert(0);
        state
            .step_target_terminal_failure_counts
            .entry(step_id)
            .or_insert(0);
    }

    Ok(state)
}

fn project_flow_run_terminal_from_machine(
    _state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    expected_phase: flow_run::Phase,
    run_status: flow_run::FlowRunStatus,
    transition_id: flow_run::TransitionId,
) -> Result<flow_run::Outcome, MobError> {
    let next_state = project_flow_run_state_from_machine(machine_state, run_id)?;
    if next_state.phase != expected_phase {
        return Err(MobError::Internal(format!(
            "MobMachine run '{run_id}' projected phase {:?}, expected {:?}",
            next_state.phase, expected_phase
        )));
    }
    Ok(flow_run::Outcome {
        transition_id,
        next_state,
        effects: vec![
            flow_run::Effect::EmitFlowRunNotice(flow_run::effects::EmitFlowRunNotice {
                run_status,
            }),
            flow_run::Effect::FlowTerminalized(flow_run::effects::FlowTerminalized { run_status }),
        ],
    })
}

fn project_flow_run_step_status_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    expected_status: flow_run::StepRunStatus,
    transition_id: flow_run::TransitionId,
) -> Result<flow_run::Outcome, MobError> {
    let key = mob_dsl::RunStepKey::from(format!("{run_id}\u{0}{}", step_id.as_str()));
    let Some(status) = machine_state.run_step_status_flat.get(&key) else {
        return Err(MobError::Internal(format!(
            "MobMachine run_step_status_flat missing accepted projection for run '{run_id}' step '{step_id}'"
        )));
    };
    let projected_status = project_step_run_status(*status);
    if projected_status != expected_status {
        return Err(MobError::Internal(format!(
            "MobMachine run_step_status_flat projected {projected_status:?} for run '{run_id}' step '{step_id}', expected {expected_status:?}"
        )));
    }
    let mut next_state = state.clone();
    next_state
        .step_status
        .insert(step_id.clone(), Some(projected_status));
    Ok(flow_run::Outcome {
        transition_id,
        next_state,
        effects: flow_run_step_status_effects(step_id, projected_status, transition_id),
    })
}

fn flow_run_step_status_effects(
    step_id: &StepId,
    projected_status: flow_run::StepRunStatus,
    transition_id: flow_run::TransitionId,
) -> Vec<flow_run::Effect> {
    let mut effects = vec![flow_run::Effect::EmitStepNotice(
        flow_run::effects::EmitStepNotice {
            step_id: step_id.clone(),
            step_status: projected_status,
        },
    )];
    if transition_id == flow_run::TransitionId::DispatchStep {
        effects.push(flow_run::Effect::AdmitStepWork(
            flow_run::effects::AdmitStepWork {
                step_id: step_id.clone(),
            },
        ));
    }
    effects
}

fn project_flow_run_completed_step_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
) -> Result<flow_run::Outcome, MobError> {
    let mut outcome = project_flow_run_step_status_from_machine(
        state,
        machine_state,
        run_id,
        step_id,
        flow_run::StepRunStatus::Completed,
        flow_run::TransitionId::CompleteStep,
    )?;
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    project_flow_run_counters_from_machine(&mut outcome.next_state, machine_state, &run_key)?;
    Ok(outcome)
}

fn project_flow_run_failed_step_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
) -> Result<flow_run::Outcome, MobError> {
    let mut outcome = project_flow_run_step_status_from_machine(
        state,
        machine_state,
        run_id,
        step_id,
        flow_run::StepRunStatus::Failed,
        flow_run::TransitionId::FailStep,
    )?;
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    project_flow_run_counters_from_machine(&mut outcome.next_state, machine_state, &run_key)?;
    outcome.effects.push(flow_run::Effect::AppendFailureLedger(
        flow_run::effects::AppendFailureLedger {
            step_id: step_id.clone(),
        },
    ));
    Ok(outcome)
}

fn project_flow_run_frame_step_status_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    frame_id: &FrameId,
    node_id: &FlowNodeId,
    append_failure_ledger: bool,
) -> Result<flow_run::Outcome, MobError> {
    let expected_status =
        machine_projected_frame_step_status(machine_state, run_id, step_id, frame_id, node_id)?;
    let mut outcome = project_flow_run_step_status_from_machine(
        state,
        machine_state,
        run_id,
        step_id,
        expected_status,
        flow_run::TransitionId::ProjectFrameStepStatus,
    )?;
    match expected_status {
        flow_run::StepRunStatus::Failed => {
            let run_key = mob_dsl::RunId::from(run_id.to_string());
            project_flow_run_counters_from_machine(
                &mut outcome.next_state,
                machine_state,
                &run_key,
            )?;
            if append_failure_ledger {
                outcome.effects.push(flow_run::Effect::AppendFailureLedger(
                    flow_run::effects::AppendFailureLedger {
                        step_id: step_id.clone(),
                    },
                ));
            }
            if outcome.next_state.escalation_threshold > 0
                && outcome.next_state.consecutive_failure_count
                    >= outcome.next_state.escalation_threshold
            {
                outcome.effects.push(flow_run::Effect::EscalateSupervisor(
                    flow_run::effects::EscalateSupervisor {
                        step_id: step_id.clone(),
                    },
                ));
            }
        }
        flow_run::StepRunStatus::Completed => {
            let run_key = mob_dsl::RunId::from(run_id.to_string());
            project_flow_run_counters_from_machine(
                &mut outcome.next_state,
                machine_state,
                &run_key,
            )?;
        }
        _ => {}
    }
    Ok(outcome)
}

fn machine_projected_frame_step_status(
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    frame_id: &FrameId,
    node_id: &FlowNodeId,
) -> Result<flow_run::StepRunStatus, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    let node_key = mob_dsl::FlowNodeId::from(node_id.as_str());

    if machine_state.frame_run.get(&frame_key) != Some(&run_key) {
        return Err(MobError::Internal(format!(
            "MobMachine ProjectFrameStepStatus accepted frame '{frame_id}' that does not belong \
             to run '{run_id}'"
        )));
    }
    let Some(step_ids) = machine_state.frame_node_step_ids.get(&frame_key) else {
        return Err(MobError::Internal(format!(
            "MobMachine ProjectFrameStepStatus missing step mapping for frame '{frame_id}'"
        )));
    };
    if step_ids.get(&node_key) != Some(&step_key) {
        return Err(MobError::Internal(format!(
            "MobMachine ProjectFrameStepStatus mapped frame '{frame_id}' node '{node_id}' to a \
             different step than '{step_id}'"
        )));
    }
    let Some(node_statuses) = machine_state.frame_node_status.get(&frame_key) else {
        return Err(MobError::Internal(format!(
            "MobMachine ProjectFrameStepStatus missing node statuses for frame '{frame_id}'"
        )));
    };
    let Some(node_status) = node_statuses.get(&node_key).copied() else {
        return Err(MobError::Internal(format!(
            "MobMachine ProjectFrameStepStatus missing status for frame '{frame_id}' node \
             '{node_id}'"
        )));
    };
    match node_status {
        mob_dsl::NodeRunStatus::Completed => Ok(flow_run::StepRunStatus::Completed),
        mob_dsl::NodeRunStatus::Skipped => Ok(flow_run::StepRunStatus::Skipped),
        mob_dsl::NodeRunStatus::Failed => Ok(flow_run::StepRunStatus::Failed),
        other => Err(MobError::Internal(format!(
            "MobMachine ProjectFrameStepStatus accepted non-projectable frame node status \
             {other:?} for frame '{frame_id}' node '{node_id}'"
        ))),
    }
}

fn project_flow_run_counters_from_machine(
    state: &mut flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_key: &mob_dsl::RunId,
) -> Result<(), MobError> {
    state.failure_count = required_machine_value(
        &machine_state.run_failure_count,
        run_key,
        "run_failure_count",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    state.consecutive_failure_count = required_machine_value(
        &machine_state.run_consecutive_failure_count,
        run_key,
        "run_consecutive_failure_count",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    Ok(())
}

fn project_flow_run_step_output_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
) -> Result<flow_run::Outcome, MobError> {
    let key = mob_dsl::RunStepKey::from(format!("{run_id}\u{0}{}", step_id.as_str()));
    let recorded = machine_state
        .run_output_recorded_flat
        .get(&key)
        .copied()
        .or_else(|| {
            machine_state
                .run_output_recorded
                .get(&mob_dsl::RunId::from(run_id.to_string()))
                .and_then(|outputs| outputs.get(&mob_dsl::StepId::from(step_id.as_str())))
                .copied()
        })
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine output projection missing accepted record for run '{run_id}' step '{step_id}'"
            ))
        })?;
    if !recorded {
        return Err(MobError::Internal(format!(
            "MobMachine output projection for run '{run_id}' step '{step_id}' was false"
        )));
    }
    let mut next_state = state.clone();
    next_state.output_recorded.insert(step_id.clone(), true);
    Ok(flow_run::Outcome {
        transition_id: flow_run::TransitionId::RecordStepOutput,
        next_state,
        effects: vec![flow_run::Effect::PersistStepOutput(
            flow_run::effects::PersistStepOutput {
                step_id: step_id.clone(),
            },
        )],
    })
}

fn project_flow_run_condition_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    expected: bool,
    transition_id: flow_run::TransitionId,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    let flat_key = mob_dsl::RunStepKey::from(format!("{run_id}\u{0}{}", step_id.as_str()));
    let projected = machine_state
        .run_step_condition_results_flat
        .get(&flat_key)
        .copied()
        .flatten()
        .or_else(|| {
            machine_state
        .run_step_condition_results
        .get(&run_key)
        .and_then(|conditions| conditions.get(&step_key))
        .copied()
        .flatten()
        })
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine condition projection missing accepted result for run '{run_id}' step '{step_id}'"
            ))
        })?;
    if projected != expected {
        return Err(MobError::Internal(format!(
            "MobMachine condition projection for run '{run_id}' step '{step_id}' was {projected}, expected {expected}"
        )));
    }
    let mut next_state = state.clone();
    next_state
        .step_condition_results
        .insert(step_id.clone(), Some(projected));
    Ok(flow_run::Outcome {
        transition_id,
        next_state,
        effects: Vec::new(),
    })
}

fn project_flow_run_target_count_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    expected_count: u32,
) -> Result<flow_run::Outcome, MobError> {
    let count = projected_run_step_value(
        Some(&machine_state.run_step_target_counts_flat),
        &machine_state.run_step_target_counts,
        run_id,
        step_id,
        "run_step_target_counts",
    )?;
    if count != u64::from(expected_count) {
        return Err(MobError::Internal(format!(
            "MobMachine target count projection for run '{run_id}' step '{step_id}' was {count}, expected {expected_count}"
        )));
    }
    let mut next_state = state.clone();
    next_state
        .step_target_counts
        .insert(step_id.clone(), expected_count);
    Ok(flow_run::Outcome {
        transition_id: flow_run::TransitionId::RegisterTargets,
        next_state,
        effects: Vec::new(),
    })
}

fn project_flow_run_target_success_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    target_id: &MeerkatId,
) -> Result<flow_run::Outcome, MobError> {
    let count = projected_run_step_value(
        Some(&machine_state.run_step_target_success_counts_flat),
        &machine_state.run_step_target_success_counts,
        run_id,
        step_id,
        "run_step_target_success_counts",
    )?;
    let mut next_state = state.clone();
    next_state
        .step_target_success_counts
        .insert(step_id.clone(), saturating_u64_to_u32(count));
    Ok(flow_run::Outcome {
        transition_id: flow_run::TransitionId::RecordTargetSuccess,
        next_state,
        effects: vec![flow_run::Effect::ProjectTargetSuccess(
            flow_run::effects::ProjectTargetSuccess {
                step_id: step_id.clone(),
                target_id: target_id.clone(),
            },
        )],
    })
}

fn project_flow_run_target_terminal_failure_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
) -> Result<flow_run::Outcome, MobError> {
    let count = projected_run_step_value(
        Some(&machine_state.run_step_target_terminal_failure_counts_flat),
        &machine_state.run_step_target_terminal_failure_counts,
        run_id,
        step_id,
        "run_step_target_terminal_failure_counts",
    )?;
    let mut next_state = state.clone();
    next_state
        .step_target_terminal_failure_counts
        .insert(step_id.clone(), saturating_u64_to_u32(count));
    Ok(flow_run::Outcome {
        transition_id: flow_run::TransitionId::RecordTargetTerminalFailure,
        next_state,
        effects: Vec::new(),
    })
}

fn project_flow_run_target_canceled_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    target_id: &MeerkatId,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    required_machine_value(&machine_state.run_status, &run_key, "run_status")?;
    let mut next_state = state.clone();
    next_state
        .step_status
        .entry(step_id.clone())
        .or_insert(None);
    Ok(flow_run::Outcome {
        transition_id: flow_run::TransitionId::RecordTargetCanceled,
        next_state,
        effects: vec![flow_run::Effect::ProjectTargetCanceled(
            flow_run::effects::ProjectTargetCanceled {
                step_id: step_id.clone(),
                target_id: target_id.clone(),
            },
        )],
    })
}

fn project_flow_run_target_failure_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    target_id: &MeerkatId,
    retry_key: &str,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let flat_key = mob_dsl::RunStepKey::from(format!("{run_id}\u{0}{}", step_id.as_str()));
    let count = machine_state
        .run_target_retry_counts_flat
        .get(&flat_key)
        .copied()
        .or_else(|| {
            machine_state
                .run_target_retry_counts
                .get(&run_key)
                .and_then(|counts| counts.get(retry_key))
                .copied()
        })
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine target retry projection missing key '{retry_key}' for run '{run_id}'"
            ))
        })?;
    let mut next_state = state.clone();
    next_state
        .target_retry_counts
        .insert(retry_key.to_owned(), saturating_u64_to_u32(count));
    Ok(flow_run::Outcome {
        transition_id: flow_run::TransitionId::RecordTargetFailure,
        next_state,
        effects: vec![flow_run::Effect::ProjectTargetFailure(
            flow_run::effects::ProjectTargetFailure {
                step_id: step_id.clone(),
                target_id: target_id.clone(),
            },
        )],
    })
}

fn project_flow_run_ready_frames_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    transition_id: flow_run::TransitionId,
    last_granted_frame: Option<FrameId>,
    expected_frame: Option<&FrameId>,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let ready_frames = machine_state
        .run_ready_frames
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_frame_id)
        .collect::<Vec<_>>();
    let mut ready_frame_membership = machine_state
        .run_ready_frame_membership
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_frame_id)
        .collect::<BTreeSet<_>>();
    for frame_id in &machine_state.run_ready_frame_membership_flat {
        if machine_state.frame_run.get(frame_id) == Some(&run_key) {
            ready_frame_membership.insert(project_frame_id(frame_id));
        }
    }
    let ready_frames = if ready_frame_membership.is_empty() {
        ready_frames
    } else {
        ready_frame_membership.iter().cloned().collect()
    };
    if let Some(expected_frame) = expected_frame {
        required_machine_value(&machine_state.run_status, &run_key, "run_status")?;
        if transition_id == flow_run::TransitionId::RegisterReadyFrame
            && !ready_frame_membership.contains(expected_frame)
        {
            return Err(MobError::Internal(format!(
                "MobMachine ready-frame projection did not contain frame '{expected_frame}' for run '{run_id}'"
            )));
        }
    }
    let mut next_state = state.clone();
    next_state.ready_frames = ready_frames;
    next_state.ready_frame_membership = ready_frame_membership;
    next_state.active_node_count = required_machine_value(
        &machine_state.run_active_node_count,
        &run_key,
        "run_active_node_count",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    if let Some(last_granted_frame) = last_granted_frame.or_else(|| {
        machine_state
            .run_last_granted_frame
            .get(&run_key)
            .map(project_frame_id)
    }) {
        next_state.last_granted_frame = last_granted_frame;
    }
    Ok(flow_run::Outcome {
        transition_id,
        next_state,
        effects: Vec::new(),
    })
}

fn project_flow_run_pending_body_frames_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    transition_id: flow_run::TransitionId,
    last_granted_loop: Option<LoopInstanceId>,
    expected_loop: Option<&LoopInstanceId>,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let pending_body_frame_loops = machine_state
        .run_pending_body_frame_loops
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_loop_instance_id)
        .collect::<Vec<_>>();
    let mut pending_body_frame_loop_membership = machine_state
        .run_pending_body_frame_loop_membership
        .get(&run_key)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(project_loop_instance_id)
        .collect::<BTreeSet<_>>();
    for loop_instance_id in &machine_state.run_pending_body_frame_loop_membership_flat {
        if machine_state
            .loop_parent_frame
            .get(loop_instance_id)
            .and_then(|frame_id| machine_state.frame_run.get(frame_id))
            == Some(&run_key)
        {
            pending_body_frame_loop_membership.insert(project_loop_instance_id(loop_instance_id));
        }
    }
    let pending_body_frame_loops = if pending_body_frame_loop_membership.is_empty() {
        pending_body_frame_loops
    } else {
        pending_body_frame_loop_membership.iter().cloned().collect()
    };
    if let Some(expected_loop) = expected_loop
        && transition_id == flow_run::TransitionId::RegisterPendingBodyFrame
        && !pending_body_frame_loop_membership.contains(expected_loop)
    {
        return Err(MobError::Internal(format!(
            "MobMachine pending-body-frame projection did not contain loop '{expected_loop}' for run '{run_id}'"
        )));
    }
    let mut next_state = state.clone();
    next_state.pending_body_frame_loops = pending_body_frame_loops;
    next_state.pending_body_frame_loop_membership = pending_body_frame_loop_membership;
    next_state.active_frame_count = required_machine_value(
        &machine_state.run_active_frame_count,
        &run_key,
        "run_active_frame_count",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    if let Some(last_granted_loop) = last_granted_loop.or_else(|| {
        machine_state
            .run_last_granted_loop
            .get(&run_key)
            .map(project_loop_instance_id)
    }) {
        next_state.last_granted_loop = last_granted_loop;
    }
    Ok(flow_run::Outcome {
        transition_id,
        next_state,
        effects: Vec::new(),
    })
}

fn project_flow_run_node_scheduler_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    required_machine_value(&machine_state.run_status, &run_key, "run_status")?;
    let granted = machine_state
        .run_last_granted_frame
        .get(&run_key)
        .map(project_frame_id)
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine node scheduler projection did not grant a frame for run '{run_id}'"
            ))
        })?;
    let mut outcome = project_flow_run_ready_frames_from_machine(
        state,
        machine_state,
        run_id,
        flow_run::TransitionId::PumpNodeScheduler,
        Some(granted.clone()),
        None,
    )?;
    outcome.effects.push(flow_run::Effect::GrantNodeSlot(
        flow_run::effects::GrantNodeSlot { frame_id: granted },
    ));
    Ok(outcome)
}

fn project_flow_run_frame_scheduler_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
) -> Result<flow_run::Outcome, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    required_machine_value(&machine_state.run_status, &run_key, "run_status")?;
    let granted = machine_state
        .run_last_granted_loop
        .get(&run_key)
        .map(project_loop_instance_id)
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine frame scheduler projection did not grant a loop for run '{run_id}'"
            ))
        })?;
    let mut outcome = project_flow_run_pending_body_frames_from_machine(
        state,
        machine_state,
        run_id,
        flow_run::TransitionId::PumpFrameScheduler,
        Some(granted.clone()),
        None,
    )?;
    outcome.effects.push(flow_run::Effect::GrantBodyFrameStart(
        flow_run::effects::GrantBodyFrameStart {
            loop_instance_id: granted,
        },
    ));
    Ok(outcome)
}

fn projected_run_step_value<V: Copy>(
    flat_map: Option<&BTreeMap<mob_dsl::RunStepKey, V>>,
    map: &BTreeMap<mob_dsl::RunId, BTreeMap<mob_dsl::StepId, V>>,
    run_id: &RunId,
    step_id: &StepId,
    field: &'static str,
) -> Result<V, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    let flat_key = mob_dsl::RunStepKey::from(format!("{run_id}\u{0}{}", step_id.as_str()));
    flat_map
        .and_then(|values| values.get(&flat_key))
        .copied()
        .or_else(|| {
            map.get(&run_key)
                .and_then(|values| values.get(&step_key))
                .copied()
        })
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field {field} missing run '{run_id}' step '{step_id}'"
            ))
        })
}

fn project_flow_frame_seed_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
    transition_id: flow_frame::TransitionId,
) -> Result<flow_frame::Outcome, MobError> {
    let state = project_flow_frame_state_from_machine(machine_state, frame_id, BTreeSet::new())?;
    Ok(flow_frame::Outcome {
        transition_id,
        next_state: state,
        effects: Vec::new(),
    })
}

pub(crate) fn project_flow_frame_state_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
    branch_winners: BTreeSet<BranchId>,
) -> Result<flow_frame::State, MobError> {
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    let phase = match required_machine_value(&machine_state.frame_phase, &frame_key, "frame_phase")?
    {
        mob_dsl::FrameStatus::Running => flow_frame::Phase::Running,
        mob_dsl::FrameStatus::Completed => flow_frame::Phase::Completed,
        mob_dsl::FrameStatus::Failed => flow_frame::Phase::Failed,
        mob_dsl::FrameStatus::Canceled => flow_frame::Phase::Canceled,
    };
    let frame_scope =
        match required_machine_value(&machine_state.frame_scope, &frame_key, "frame_scope")? {
            mob_dsl::FrameScope::Root => flow_frame::FrameScope::Root,
            mob_dsl::FrameScope::Body => flow_frame::FrameScope::Body,
        };
    let loop_instance_id = machine_state
        .frame_parent_loop
        .get(&frame_key)
        .and_then(Clone::clone)
        .map(|id| project_loop_instance_id(&id))
        .unwrap_or_else(|| LoopInstanceId::from(String::new()));
    let iteration = *required_machine_value(
        &machine_state.frame_iteration,
        &frame_key,
        "frame_iteration",
    )?;
    let tracked_nodes = required_machine_value(
        &machine_state.frame_tracked_nodes,
        &frame_key,
        "frame_tracked_nodes",
    )?
    .iter()
    .map(project_flow_node_id)
    .collect::<BTreeSet<_>>();
    let ordered_nodes = required_machine_value(
        &machine_state.frame_ordered_nodes,
        &frame_key,
        "frame_ordered_nodes",
    )?
    .iter()
    .map(project_flow_node_id)
    .collect::<Vec<_>>();
    let mut state = flow_frame::State {
        phase,
        frame_id: frame_id.clone(),
        frame_scope,
        loop_instance_id,
        iteration,
        last_admitted_node: machine_state
            .frame_last_admitted_node
            .get(&frame_key)
            .map(project_flow_node_id)
            .unwrap_or_else(|| FlowNodeId::from(String::new())),
        tracked_nodes,
        ordered_nodes,
        node_kind: project_node_map(
            required_machine_value(
                &machine_state.frame_node_kind,
                &frame_key,
                "frame_node_kind",
            )?
            .clone(),
            project_flow_node_kind,
        ),
        node_dependencies: project_node_map(
            required_machine_value(
                &machine_state.frame_node_dependencies,
                &frame_key,
                "frame_node_dependencies",
            )?
            .clone(),
            |deps| deps.iter().map(project_flow_node_id).collect(),
        ),
        node_dependency_modes: project_node_map(
            required_machine_value(
                &machine_state.frame_node_dependency_modes,
                &frame_key,
                "frame_node_dependency_modes",
            )?
            .clone(),
            project_flow_frame_dependency_mode,
        ),
        node_branches: project_node_map(
            required_machine_value(
                &machine_state.frame_node_branches,
                &frame_key,
                "frame_node_branches",
            )?
            .clone(),
            |branch| branch.as_ref().map(project_branch_id),
        ),
        branch_winners,
        node_status: project_node_map(
            machine_state
                .frame_node_status
                .get(&frame_key)
                .cloned()
                .unwrap_or_default(),
            project_node_run_status,
        ),
        ready_queue: machine_state
            .frame_ready_queue
            .get(&frame_key)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(project_flow_node_id)
            .collect(),
        output_recorded: project_node_map(
            machine_state
                .frame_output_recorded
                .get(&frame_key)
                .cloned()
                .unwrap_or_default(),
            |v| v,
        ),
        node_condition_results: project_node_map(
            machine_state
                .frame_node_condition_results
                .get(&frame_key)
                .cloned()
                .unwrap_or_default(),
            |v| v,
        ),
    };
    for node_id in state.tracked_nodes.clone() {
        let node_key = mob_dsl::FrameNodeKey::from(format!("{frame_id}\u{0}{}", node_id.as_str()));
        if let Some(recorded) = machine_state
            .frame_output_recorded_flat
            .get(&node_key)
            .copied()
        {
            state.output_recorded.insert(node_id, recorded);
        }
    }
    for node_id in state.tracked_nodes.clone() {
        state
            .node_status
            .entry(node_id.clone())
            .or_insert(flow_frame::NodeRunStatus::Pending);
        state
            .output_recorded
            .entry(node_id.clone())
            .or_insert(false);
        state.node_condition_results.entry(node_id).or_insert(None);
    }
    for node_id in state.ready_queue.clone() {
        if state.node_status.get(&node_id) == Some(&flow_frame::NodeRunStatus::Pending) {
            state
                .node_status
                .insert(node_id, flow_frame::NodeRunStatus::Ready);
        }
    }
    Ok(state)
}

pub(crate) fn project_flow_frame_authority_state_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
) -> Result<flow_frame::State, MobError> {
    let state = project_flow_frame_state_from_machine(machine_state, frame_id, BTreeSet::new())?;
    let branch_winners = state
        .node_status
        .iter()
        .filter(|(_, status)| **status == flow_frame::NodeRunStatus::Completed)
        .filter_map(|(node_id, _)| state.node_branches.get(node_id).and_then(Clone::clone))
        .collect();
    project_flow_frame_state_from_machine(machine_state, frame_id, branch_winners)
}

fn project_flow_frame_admit_from_machine(
    state: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
) -> Result<flow_frame::Outcome, MobError> {
    let next_state = project_flow_frame_state_from_machine(
        machine_state,
        &state.frame_id,
        state.branch_winners.clone(),
    )?;
    let admitted_node = if !next_state.last_admitted_node.as_str().is_empty() {
        next_state.last_admitted_node.clone()
    } else {
        state
            .node_status
            .iter()
            .find_map(|(node_id, previous)| {
                let next = next_state.node_status.get(node_id)?;
                (*previous == flow_frame::NodeRunStatus::Ready
                    && *next == flow_frame::NodeRunStatus::Running)
                    .then(|| node_id.clone())
            })
            .or_else(|| {
                next_state.node_status.iter().find_map(|(node_id, next)| {
                    (*next == flow_frame::NodeRunStatus::Running
                        && state.node_status.get(node_id)
                            != Some(&flow_frame::NodeRunStatus::Running))
                    .then(|| node_id.clone())
                })
            })
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "MobMachine frame '{}' did not project an admitted running node",
                    state.frame_id
                ))
            })?
    };
    let effect = match next_state.node_kind.get(&admitted_node).copied() {
        Some(flow_frame::FlowNodeKind::Step) => {
            flow_frame::Effect::AdmitStepWork(flow_frame::effects::AdmitStepWork {
                frame_id: state.frame_id.clone(),
                node_id: admitted_node,
            })
        }
        Some(flow_frame::FlowNodeKind::Loop) => {
            flow_frame::Effect::StartLoopNode(flow_frame::effects::StartLoopNode {
                frame_id: state.frame_id.clone(),
                node_id: admitted_node,
            })
        }
        None => {
            return Err(MobError::Internal(format!(
                "MobMachine frame '{}' admitted unknown node '{}'",
                state.frame_id, admitted_node
            )));
        }
    };
    Ok(flow_frame::Outcome {
        transition_id: flow_frame::TransitionId::AdmitNextReadyNode,
        next_state,
        effects: vec![effect],
    })
}

fn project_flow_frame_node_from_machine(
    state: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
    node_id: &FlowNodeId,
    expected_status: flow_frame::NodeRunStatus,
    transition_id: flow_frame::TransitionId,
) -> Result<flow_frame::Outcome, MobError> {
    let next_state = project_flow_frame_state_from_machine(
        machine_state,
        &state.frame_id,
        state.branch_winners.clone(),
    )?;
    let projected = next_state
        .node_status
        .get(node_id)
        .copied()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine frame '{}' missing node '{}' status projection",
                state.frame_id, node_id
            ))
        })?;
    if projected != expected_status {
        return Err(MobError::Internal(format!(
            "MobMachine frame '{}' projected node '{}' as {:?}, expected {:?}",
            state.frame_id, node_id, projected, expected_status
        )));
    }
    Ok(flow_frame::Outcome {
        transition_id,
        next_state,
        effects: vec![flow_frame::Effect::NodeExecutionReleased(
            flow_frame::effects::NodeExecutionReleased {
                frame_id: state.frame_id.clone(),
                node_id: node_id.clone(),
            },
        )],
    })
}

fn project_flow_frame_node_output_from_machine(
    state: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
    node_id: &FlowNodeId,
) -> Result<flow_frame::Outcome, MobError> {
    let next_state = project_flow_frame_state_from_machine(
        machine_state,
        &state.frame_id,
        state.branch_winners.clone(),
    )?;
    if next_state.output_recorded.get(node_id) != Some(&true) {
        return Err(MobError::Internal(format!(
            "MobMachine frame '{}' did not project output recorded for node '{}'",
            state.frame_id, node_id
        )));
    }
    Ok(flow_frame::Outcome {
        transition_id: flow_frame::TransitionId::RecordNodeOutput,
        effects: vec![flow_frame::Effect::PersistStepOutput(
            flow_frame::effects::PersistStepOutput {
                frame_id: state.frame_id.clone(),
                node_id: node_id.clone(),
            },
        )],
        next_state,
    })
}

fn project_flow_frame_seal_from_machine(
    state: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
    terminal_status: flow_frame::FrameTerminalStatus,
) -> Result<flow_frame::Outcome, MobError> {
    let frame_key = mob_dsl::FrameId::from(state.frame_id.as_str());
    let projected_phase =
        match required_machine_value(&machine_state.frame_phase, &frame_key, "frame_phase")? {
            mob_dsl::FrameStatus::Running => flow_frame::Phase::Running,
            mob_dsl::FrameStatus::Completed => flow_frame::Phase::Completed,
            mob_dsl::FrameStatus::Failed => flow_frame::Phase::Failed,
            mob_dsl::FrameStatus::Canceled => flow_frame::Phase::Canceled,
        };
    let expected_phase = match terminal_status {
        flow_frame::FrameTerminalStatus::Completed => flow_frame::Phase::Completed,
        flow_frame::FrameTerminalStatus::Failed => flow_frame::Phase::Failed,
        flow_frame::FrameTerminalStatus::Canceled => flow_frame::Phase::Canceled,
    };
    if projected_phase != expected_phase {
        return Err(MobError::Internal(format!(
            "MobMachine frame '{}' projected phase {:?}, expected {:?}",
            state.frame_id, projected_phase, expected_phase
        )));
    }

    let mut next_state = state.clone();
    next_state.phase = projected_phase;
    let effect = match (state.frame_scope, terminal_status) {
        (flow_frame::FrameScope::Root, flow_frame::FrameTerminalStatus::Completed) => {
            flow_frame::Effect::RootFrameCompleted(flow_frame::effects::RootFrameCompleted {
                frame_id: state.frame_id.clone(),
            })
        }
        (flow_frame::FrameScope::Root, flow_frame::FrameTerminalStatus::Failed) => {
            flow_frame::Effect::RootFrameFailed(flow_frame::effects::RootFrameFailed {
                frame_id: state.frame_id.clone(),
            })
        }
        (flow_frame::FrameScope::Root, flow_frame::FrameTerminalStatus::Canceled) => {
            flow_frame::Effect::RootFrameCanceled(flow_frame::effects::RootFrameCanceled {
                frame_id: state.frame_id.clone(),
            })
        }
        (flow_frame::FrameScope::Body, flow_frame::FrameTerminalStatus::Completed) => {
            flow_frame::Effect::BodyFrameCompleted(flow_frame::effects::BodyFrameCompleted {
                frame_id: state.frame_id.clone(),
                loop_instance_id: state.loop_instance_id.clone(),
                iteration: state.iteration,
            })
        }
        (flow_frame::FrameScope::Body, flow_frame::FrameTerminalStatus::Failed) => {
            flow_frame::Effect::BodyFrameFailed(flow_frame::effects::BodyFrameFailed {
                frame_id: state.frame_id.clone(),
                loop_instance_id: state.loop_instance_id.clone(),
                iteration: state.iteration,
            })
        }
        (flow_frame::FrameScope::Body, flow_frame::FrameTerminalStatus::Canceled) => {
            flow_frame::Effect::BodyFrameCanceled(flow_frame::effects::BodyFrameCanceled {
                frame_id: state.frame_id.clone(),
                loop_instance_id: state.loop_instance_id.clone(),
                iteration: state.iteration,
            })
        }
    };

    Ok(flow_frame::Outcome {
        transition_id: flow_frame::TransitionId::SealFrame,
        next_state,
        effects: vec![effect],
    })
}

fn project_loop_iteration_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    loop_instance_id: &LoopInstanceId,
    transition_id: loop_iteration::TransitionId,
    effects: Vec<loop_iteration::Effect>,
) -> Result<loop_iteration::Outcome, MobError> {
    Ok(loop_iteration::Outcome {
        transition_id,
        next_state: project_loop_iteration_state_from_machine(machine_state, loop_instance_id)?,
        effects,
    })
}

fn project_loop_iteration_state_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    loop_instance_id: &LoopInstanceId,
) -> Result<loop_iteration::State, MobError> {
    let loop_key = mob_dsl::LoopInstanceId::from(loop_instance_id.as_str());
    let phase = match required_machine_value(&machine_state.loop_phase, &loop_key, "loop_phase")? {
        mob_dsl::LoopStatus::Running => loop_iteration::Phase::Running,
        mob_dsl::LoopStatus::Completed => loop_iteration::Phase::Completed,
        mob_dsl::LoopStatus::Exhausted => loop_iteration::Phase::Exhausted,
        mob_dsl::LoopStatus::Failed => loop_iteration::Phase::Failed,
        mob_dsl::LoopStatus::Canceled => loop_iteration::Phase::Canceled,
    };
    let stage = match required_machine_value(&machine_state.loop_stage, &loop_key, "loop_stage")? {
        mob_dsl::LoopIterationStage::AwaitingBodyFrame => {
            loop_iteration::LoopIterationStage::AwaitingBodyFrame
        }
        mob_dsl::LoopIterationStage::BodyFrameActive => {
            loop_iteration::LoopIterationStage::BodyFrameActive
        }
        mob_dsl::LoopIterationStage::AwaitingUntilEvaluation => {
            loop_iteration::LoopIterationStage::AwaitingUntilEvaluation
        }
    };
    Ok(loop_iteration::State {
        phase,
        loop_instance_id: loop_instance_id.clone(),
        parent_frame_id: project_frame_id(required_machine_value(
            &machine_state.loop_parent_frame,
            &loop_key,
            "loop_parent_frame",
        )?),
        parent_node_id: project_flow_node_id(required_machine_value(
            &machine_state.loop_parent_node,
            &loop_key,
            "loop_parent_node",
        )?),
        loop_id: project_loop_id(required_machine_value(
            &machine_state.loop_definition,
            &loop_key,
            "loop_definition",
        )?),
        depth: *required_machine_value(&machine_state.loop_depth, &loop_key, "loop_depth")?,
        stage,
        current_iteration: u32::try_from(*required_machine_value(
            &machine_state.loop_current_iteration,
            &loop_key,
            "loop_current_iteration",
        )?)
        .map_err(|_| MobError::Internal("loop current_iteration exceeds u32".to_string()))?,
        last_completed_iteration: u32::try_from(*required_machine_value(
            &machine_state.loop_last_completed_iteration,
            &loop_key,
            "loop_last_completed_iteration",
        )?)
        .map_err(|_| MobError::Internal("loop last_completed_iteration exceeds u32".to_string()))?,
        max_iterations: u32::try_from(*required_machine_value(
            &machine_state.loop_max_iterations,
            &loop_key,
            "loop_max_iterations",
        )?)
        .map_err(|_| MobError::Internal("loop max_iterations exceeds u32".to_string()))?,
        active_body_frame_id: machine_state
            .loop_active_body_frame
            .get(&loop_key)
            .cloned()
            .flatten()
            .map(|frame_id| project_frame_id(&frame_id)),
    })
}

fn required_machine_value<'a, K, V>(
    map: &'a BTreeMap<K, V>,
    key: &K,
    field: &'static str,
) -> Result<&'a V, MobError>
where
    K: Ord + std::fmt::Debug,
{
    map.get(key).ok_or_else(|| {
        MobError::Internal(format!(
            "MobMachine projection field {field} missing key {key:?}"
        ))
    })
}

fn project_step_map<T, U>(
    input: BTreeMap<mob_dsl::StepId, T>,
    mut f: impl FnMut(T) -> U,
) -> BTreeMap<StepId, U> {
    input
        .into_iter()
        .map(|(step_id, value)| (project_step_id(&step_id), f(value)))
        .collect()
}

fn project_node_map<T, U>(
    input: BTreeMap<mob_dsl::FlowNodeId, T>,
    mut f: impl FnMut(T) -> U,
) -> BTreeMap<FlowNodeId, U> {
    input
        .into_iter()
        .map(|(node_id, value)| (project_flow_node_id(&node_id), f(value)))
        .collect()
}

fn project_step_option_status_map(
    input: BTreeMap<mob_dsl::StepId, Option<mob_dsl::StepRunStatus>>,
) -> BTreeMap<StepId, Option<flow_run::StepRunStatus>> {
    project_step_map(input, |status| status.map(project_step_run_status))
}

fn project_step_id(step_id: &mob_dsl::StepId) -> StepId {
    StepId::from(step_id.as_str())
}

fn project_run_step_key_for_run(key: &mob_dsl::RunStepKey, run_id: &RunId) -> Option<StepId> {
    let (projected_run_id, step_id) = key.0.split_once('\0')?;
    (projected_run_id == run_id.to_string()).then(|| StepId::from(step_id))
}

fn saturating_u64_to_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn project_frame_id(frame_id: &mob_dsl::FrameId) -> FrameId {
    FrameId::from(frame_id.as_str())
}

fn project_loop_instance_id(loop_instance_id: &mob_dsl::LoopInstanceId) -> LoopInstanceId {
    LoopInstanceId::from(loop_instance_id.as_str())
}

fn project_loop_id(loop_id: &mob_dsl::LoopId) -> LoopId {
    LoopId::from(loop_id.0.as_str())
}

fn project_flow_node_id(node_id: &mob_dsl::FlowNodeId) -> FlowNodeId {
    FlowNodeId::from(node_id.0.as_str())
}

fn project_branch_id(branch_id: &mob_dsl::BranchId) -> BranchId {
    BranchId::from(branch_id.0.as_str())
}

fn project_flow_run_dependency_mode(mode: mob_dsl::DependencyMode) -> flow_run::DependencyMode {
    match mode {
        mob_dsl::DependencyMode::All => flow_run::DependencyMode::All,
        mob_dsl::DependencyMode::Any => flow_run::DependencyMode::Any,
    }
}

fn project_flow_frame_dependency_mode(mode: mob_dsl::DependencyMode) -> flow_frame::DependencyMode {
    match mode {
        mob_dsl::DependencyMode::All => flow_frame::DependencyMode::All,
        mob_dsl::DependencyMode::Any => flow_frame::DependencyMode::Any,
    }
}

fn project_collection_policy(
    policy: mob_dsl::CollectionPolicyKind,
) -> flow_run::CollectionPolicyKind {
    match policy {
        mob_dsl::CollectionPolicyKind::All => flow_run::CollectionPolicyKind::All,
        mob_dsl::CollectionPolicyKind::Any => flow_run::CollectionPolicyKind::Any,
        mob_dsl::CollectionPolicyKind::Quorum => flow_run::CollectionPolicyKind::Quorum,
    }
}

fn project_step_run_status(status: mob_dsl::StepRunStatus) -> flow_run::StepRunStatus {
    match status {
        mob_dsl::StepRunStatus::Dispatched => flow_run::StepRunStatus::Dispatched,
        mob_dsl::StepRunStatus::Completed => flow_run::StepRunStatus::Completed,
        mob_dsl::StepRunStatus::Failed => flow_run::StepRunStatus::Failed,
        mob_dsl::StepRunStatus::Skipped => flow_run::StepRunStatus::Skipped,
        mob_dsl::StepRunStatus::Canceled => flow_run::StepRunStatus::Canceled,
    }
}

fn project_flow_node_kind(kind: mob_dsl::FlowNodeKind) -> flow_frame::FlowNodeKind {
    match kind {
        mob_dsl::FlowNodeKind::Step => flow_frame::FlowNodeKind::Step,
        mob_dsl::FlowNodeKind::Loop => flow_frame::FlowNodeKind::Loop,
    }
}

fn project_node_run_status(status: mob_dsl::NodeRunStatus) -> flow_frame::NodeRunStatus {
    match status {
        mob_dsl::NodeRunStatus::Pending => flow_frame::NodeRunStatus::Pending,
        mob_dsl::NodeRunStatus::Ready => flow_frame::NodeRunStatus::Ready,
        mob_dsl::NodeRunStatus::Running => flow_frame::NodeRunStatus::Running,
        mob_dsl::NodeRunStatus::Completed => flow_frame::NodeRunStatus::Completed,
        mob_dsl::NodeRunStatus::Failed => flow_frame::NodeRunStatus::Failed,
        mob_dsl::NodeRunStatus::Skipped => flow_frame::NodeRunStatus::Skipped,
        mob_dsl::NodeRunStatus::Canceled => flow_frame::NodeRunStatus::Canceled,
    }
}

fn input_catalog(input: &mob_dsl::MobMachineInput) -> Result<MobMachineCatalogInput, MobError> {
    match input {
        mob_dsl::MobMachineInput::CreateRunSeed { .. } => Ok(MobMachineCatalogInput::CreateRunSeed),
        mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand { .. } => {
            Ok(MobMachineCatalogInput::AuthorizeFlowRunReducerCommand)
        }
        mob_dsl::MobMachineInput::CreateFrameSeed { .. } => {
            Ok(MobMachineCatalogInput::CreateFrameSeed)
        }
        mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand { .. } => {
            Ok(MobMachineCatalogInput::AuthorizeFlowFrameReducerCommand)
        }
        mob_dsl::MobMachineInput::CreateLoopSeed { .. } => {
            Ok(MobMachineCatalogInput::CreateLoopSeed)
        }
        mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted { .. } => {
            Ok(MobMachineCatalogInput::RecordLoopBodyFrameCompleted)
        }
        mob_dsl::MobMachineInput::RecordLoopUntilConditionMet { .. } => {
            Ok(MobMachineCatalogInput::RecordLoopUntilConditionMet)
        }
        mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed { .. } => {
            Ok(MobMachineCatalogInput::RecordLoopUntilConditionFailed)
        }
        mob_dsl::MobMachineInput::AuthorizeLoopIterationReducerCommand { .. } => {
            Ok(MobMachineCatalogInput::AuthorizeLoopIterationReducerCommand)
        }
        crate::mob_destroying_session_ingress_feedback_input_patterns!() => {
            Err(MobError::Internal(format!(
                "MobMachine input {input:?} is not a flow reducer authority input"
            )))
        }
        non_flow_reducer_authority_mob_machine_inputs!() => Err(MobError::Internal(format!(
            "MobMachine input {input:?} is not a flow reducer authority input"
        ))),
    }
}

/// Snapshot of MobMachine-owned frame projection state stored per-frame in MobRun.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub kernel_state: flow_frame::State,
}

/// Snapshot of MobMachine-owned loop projection state stored per-loop in MobRun.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoopSnapshot {
    pub kernel_state: loop_iteration::State,
}

/// Ledger entry recording the mapping of a loop iteration to its body frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIterationLedgerEntry {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u64,
    pub frame_id: FrameId,
}

/// Persisted collection-policy kind stored in the flow kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunCollectionPolicyKind {
    All,
    Any,
    Quorum,
}

/// Persisted flow run aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobRun {
    pub run_id: RunId,
    pub mob_id: MobId,
    pub flow_id: FlowId,
    pub status: MobRunStatus,
    pub flow_state: flow_run::State,
    pub activation_params: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub step_ledger: Vec<StepLedgerEntry>,
    pub failure_ledger: Vec<FailureLedgerEntry>,
    /// Per-frame kernel snapshots indexed by FrameId.
    #[serde(default)]
    pub frames: BTreeMap<FrameId, FrameSnapshot>,
    /// Per-loop kernel snapshots indexed by LoopInstanceId.
    #[serde(default)]
    pub loops: BTreeMap<LoopInstanceId, LoopSnapshot>,
    /// Ordered ledger of loop iteration → body frame mappings.
    #[serde(default)]
    pub loop_iteration_ledger: Vec<LoopIterationLedgerEntry>,
    /// Schema version: 0 for legacy runs, 4 for self-describing frame-aware runs.
    #[serde(default)]
    pub schema_version: u32,
    /// Root frame step outputs keyed by step_id string.
    #[serde(default)]
    pub root_step_outputs: IndexMap<StepId, serde_json::Value>,
    /// Loop iteration outputs: key=loop_id, value=per-iteration step outputs.
    ///
    /// Uses `BTreeMap` (not `IndexMap`) so that recovery reconciliation can iterate
    /// in stable key order without depending on insertion order.
    #[serde(default)]
    pub loop_iteration_outputs: BTreeMap<LoopId, Vec<IndexMap<StepId, serde_json::Value>>>,
    /// Accepted MobMachine flow-authority inputs persisted in the same atomic
    /// store operation as the derived run/frame/loop projection they authorize.
    #[serde(default)]
    pub flow_authority_inputs: Vec<FlowAuthorityInputRecord>,
}

impl MobRun {
    /// Read-only access to the run's current status.
    pub fn status(&self) -> &MobRunStatus {
        &self.status
    }

    /// Read-only access to the run's current flow state.
    pub fn flow_state(&self) -> &flow_run::State {
        &self.flow_state
    }

    /// Typed view of the kernel-owned ordered step sequence.
    pub fn ordered_steps(&self) -> Result<Vec<StepId>, MobError> {
        Ok(self.flow_state.ordered_steps.clone())
    }

    /// Typed view of the kernel-owned dependency map keyed by step id.
    pub fn step_dependencies(&self) -> Result<BTreeMap<StepId, Vec<StepId>>, MobError> {
        Ok(self
            .flow_state
            .step_dependencies
            .iter()
            .map(|(step_id, deps)| (step_id.clone(), deps.clone()))
            .collect())
    }

    /// Typed view of the kernel-owned dependency mode map keyed by step id.
    pub fn step_dependency_modes(&self) -> Result<BTreeMap<StepId, DependencyMode>, MobError> {
        self.flow_state
            .step_dependency_modes
            .iter()
            .map(|(step_id, mode)| {
                let mode = match mode.as_str() {
                    "All" => DependencyMode::All,
                    "Any" => DependencyMode::Any,
                    _ => {
                        return Err(MobError::Internal(format!(
                            "flow_run step_dependency_modes unknown DependencyMode variant `{:?}` for {} step '{}'",
                            mode, self.run_id, step_id
                        )));
                    }
                };
                Ok((step_id.clone(), mode))
            })
            .collect()
    }

    /// Typed view of the kernel-owned condition-presence map keyed by step id.
    pub fn step_has_conditions(&self) -> Result<BTreeMap<StepId, bool>, MobError> {
        Ok(self
            .flow_state
            .step_has_conditions
            .iter()
            .map(|(step_id, flag)| (step_id.clone(), *flag))
            .collect())
    }

    /// Typed view of the kernel-owned branch label map keyed by step id.
    pub fn step_branches(&self) -> Result<BTreeMap<StepId, Option<BranchId>>, MobError> {
        Ok(self
            .flow_state
            .step_branches
            .iter()
            .map(|(step_id, branch)| (step_id.clone(), branch.clone()))
            .collect())
    }

    /// Typed view of the kernel-owned collection policy kind map keyed by step id.
    pub fn step_collection_policy_kinds(
        &self,
    ) -> Result<BTreeMap<StepId, RunCollectionPolicyKind>, MobError> {
        self.flow_state
            .step_collection_policies
            .iter()
            .map(|(step_id, policy)| {
                let policy = match policy.as_str() {
                    "All" => RunCollectionPolicyKind::All,
                    "Any" => RunCollectionPolicyKind::Any,
                    "Quorum" => RunCollectionPolicyKind::Quorum,
                    _ => {
                        return Err(MobError::Internal(format!(
                            "flow_run step_collection_policies unknown CollectionPolicyKind variant `{:?}` for {} step '{}'",
                            policy, self.run_id, step_id
                        )));
                    }
                };
                Ok((step_id.clone(), policy))
            })
            .collect()
    }

    /// Typed view of the kernel-owned quorum-threshold map keyed by step id.
    pub fn step_quorum_thresholds(&self) -> Result<BTreeMap<StepId, u32>, MobError> {
        Ok(self
            .flow_state
            .step_quorum_thresholds
            .iter()
            .map(|(step_id, threshold)| (step_id.clone(), *threshold))
            .collect())
    }

    /// Typed view of the kernel-owned step status map, excluding `None` entries.
    pub fn step_status_snapshot(&self) -> Result<BTreeMap<StepId, StepRunStatus>, MobError> {
        let mut statuses = BTreeMap::new();
        for (step_key, value) in &self.flow_state.step_status {
            let Some(value) = value else {
                continue;
            };
            statuses.insert(
                step_key.clone(),
                StepRunStatus::from_flow_run_status(value.as_str(), &self.run_id)?,
            );
        }

        Ok(statuses)
    }

    /// Typed view of the kernel-owned cumulative failure counter.
    pub fn failure_count(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.failure_count)
    }

    /// Typed view of the kernel-owned consecutive-failure counter.
    pub fn consecutive_failure_count(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.consecutive_failure_count)
    }

    /// Typed view of the kernel-owned retry budget.
    pub fn max_step_retries(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.max_step_retries)
    }

    /// Typed view of the kernel-owned supervisor escalation threshold.
    pub fn escalation_threshold(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.escalation_threshold)
    }
}

impl MobRun {
    pub fn pending(
        mob_id: MobId,
        flow_id: FlowId,
        flow_state: flow_run::State,
        activation_params: serde_json::Value,
    ) -> Self {
        Self::pending_with_run_id(RunId::new(), mob_id, flow_id, flow_state, activation_params)
    }

    pub(crate) fn pending_with_run_id(
        run_id: RunId,
        mob_id: MobId,
        flow_id: FlowId,
        flow_state: flow_run::State,
        activation_params: serde_json::Value,
    ) -> Self {
        Self {
            run_id,
            mob_id,
            flow_id,
            status: MobRunStatus::Pending,
            flow_state,
            activation_params,
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
            frames: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 6,
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: BTreeMap::new(),
            flow_authority_inputs: Vec::new(),
        }
    }

    pub(crate) fn append_flow_authority_inputs(
        &mut self,
        inputs: impl IntoIterator<Item = mob_dsl::MobMachineInput>,
    ) -> Result<(), MobError> {
        let records = inputs
            .into_iter()
            .map(FlowAuthorityInputRecord::from_machine_input)
            .collect::<Result<Vec<_>, _>>()?;
        self.flow_authority_inputs.extend(records);
        Ok(())
    }

    pub(crate) fn replay_flow_authority_inputs_into(
        authority: &mut mob_dsl::MobMachineAuthority,
        inputs: &[FlowAuthorityInputRecord],
        context: &str,
    ) -> Result<(), MobError> {
        for record in inputs {
            let input = record.to_machine_input();
            let input_debug = format!("{input:?}");
            let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(
                |error| {
                    MobError::Internal(format!(
                        "MobMachine flow authority replay ({context}) rejected {input_debug}: {error}"
                    ))
                },
            )?;
            if transition.from_phase != transition.to_phase {
                authority.state.lifecycle_phase = transition.to_phase;
            }
        }
        Ok(())
    }

    pub(crate) fn validate_flow_authority_projection(&self) -> Result<(), MobError> {
        if self.flow_authority_inputs.is_empty() {
            return Ok(());
        }

        let mut authority = mob_dsl::MobMachineAuthority::new();
        authority.state.lifecycle_phase = mob_dsl::MobPhase::Running;
        Self::replay_flow_authority_inputs_into(
            &mut authority,
            &self.flow_authority_inputs,
            "recovery_validate_run_projection",
        )?;

        let projected_flow_state =
            project_flow_run_state_from_machine(&authority.state, &self.run_id)?;
        if projected_flow_state != self.flow_state {
            return Err(MobError::Internal(format!(
                "flow authority log projection mismatch for run '{}': flow_state diverged",
                self.run_id
            )));
        }

        let projected_status = mob_run_status_from_flow_phase(projected_flow_state.phase);
        if projected_status != self.status {
            return Err(MobError::Internal(format!(
                "flow authority log projection mismatch for run '{}': status {:?} != {:?}",
                self.run_id, self.status, projected_status
            )));
        }

        let run_key = mob_dsl::RunId::from(self.run_id.to_string());
        let projected_frame_ids = authority
            .state
            .frame_run
            .iter()
            .filter(|(_, frame_run_id)| *frame_run_id == &run_key)
            .map(|(frame_id, _)| project_frame_id(frame_id))
            .collect::<BTreeSet<_>>();
        let persisted_frame_ids = self.frames.keys().cloned().collect::<BTreeSet<_>>();
        if projected_frame_ids != persisted_frame_ids {
            return Err(MobError::Internal(format!(
                "flow authority log projection mismatch for run '{}': frame keyset diverged",
                self.run_id
            )));
        }

        for (frame_id, snapshot) in &self.frames {
            let projected =
                project_flow_frame_authority_state_from_machine(&authority.state, frame_id)?;
            if projected != snapshot.kernel_state {
                return Err(MobError::Internal(format!(
                    "flow authority log projection mismatch for run '{}': frame '{}' diverged",
                    self.run_id, frame_id
                )));
            }
        }

        let projected_loop_ids = authority
            .state
            .loop_phase
            .keys()
            .filter(|loop_instance_id| {
                authority
                    .state
                    .loop_parent_frame
                    .get(*loop_instance_id)
                    .and_then(|frame_id| authority.state.frame_run.get(frame_id))
                    == Some(&run_key)
            })
            .map(project_loop_instance_id)
            .collect::<BTreeSet<_>>();
        let persisted_loop_ids = self.loops.keys().cloned().collect::<BTreeSet<_>>();
        if projected_loop_ids != persisted_loop_ids {
            return Err(MobError::Internal(format!(
                "flow authority log projection mismatch for run '{}': loop keyset diverged",
                self.run_id
            )));
        }

        for (loop_instance_id, snapshot) in &self.loops {
            let projected =
                project_loop_iteration_state_from_machine(&authority.state, loop_instance_id)?;
            if projected != snapshot.kernel_state {
                return Err(MobError::Internal(format!(
                    "flow authority log projection mismatch for run '{}': loop '{}' diverged",
                    self.run_id, loop_instance_id
                )));
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn flow_state_for_config(config: &FlowRunConfig) -> Result<flow_run::State, MobError> {
        let run_id = RunId::new();
        let seed_input = Self::create_run_seed_input(&run_id, config)?;
        let mut authority = mob_dsl::MobMachineAuthority::new();
        authority.state.lifecycle_phase = mob_dsl::MobPhase::Running;
        mob_dsl::MobMachineMutator::apply(&mut authority, seed_input)
            .map_err(|error| MobError::Internal(format!("test CreateRunSeed rejected: {error}")))?;
        Self::flow_state_for_config_with_authority(&run_id, config, &authority.state)
    }

    pub(crate) fn flow_state_for_config_with_authority(
        run_id: &RunId,
        config: &FlowRunConfig,
        machine_state: &mob_dsl::MobMachineState,
    ) -> Result<flow_run::State, MobError> {
        let _ = config;
        project_flow_run_state_from_machine(machine_state, run_id)
    }

    pub(crate) fn create_run_seed_input(
        run_id: &RunId,
        config: &FlowRunConfig,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let ordered_steps = topological_steps(&config.flow_spec)?;
        Ok(mob_dsl::MobMachineInput::CreateRunSeed {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            step_ids: config
                .flow_spec
                .steps
                .keys()
                .map(|step_id| mob_dsl::StepId::from(step_id.as_str()))
                .collect(),
            ordered_steps: ordered_steps
                .iter()
                .map(|step_id| mob_dsl::StepId::from(step_id.as_str()))
                .collect(),
            step_has_conditions: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        step.condition.is_some(),
                    )
                })
                .collect(),
            step_dependencies: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        step.depends_on
                            .iter()
                            .map(|dep| mob_dsl::StepId::from(dep.as_str()))
                            .collect(),
                    )
                })
                .collect(),
            step_dependency_modes: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        dependency_mode_seed_value(step.depends_on_mode.clone()),
                    )
                })
                .collect(),
            step_branches: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        step.branch
                            .as_ref()
                            .map(|branch| mob_dsl::BranchId::from(branch.as_str())),
                    )
                })
                .collect(),
            step_collection_policies: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        collection_policy_seed_value(&step.collection_policy),
                    )
                })
                .collect(),
            step_quorum_thresholds: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    let threshold = match step.collection_policy {
                        crate::definition::CollectionPolicy::Quorum { n } => u32::from(n),
                        _ => 0,
                    };
                    (mob_dsl::StepId::from(step_id.as_str()), threshold)
                })
                .collect(),
            escalation_threshold: config
                .supervisor
                .as_ref()
                .map_or(0, |supervisor| supervisor.escalation_threshold),
            max_step_retries: config
                .limits
                .as_ref()
                .and_then(|limits| limits.max_step_retries)
                .unwrap_or(0),
            max_active_nodes: config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_nodes)
                .unwrap_or(0),
            max_active_frames: config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_frames)
                .unwrap_or(0),
            max_frame_depth: config
                .limits
                .as_ref()
                .and_then(|l| l.max_frame_depth)
                .unwrap_or(0),
        })
    }

    pub(crate) fn run_flow_input(
        run_id: &RunId,
        config: &FlowRunConfig,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let mob_dsl::MobMachineInput::CreateRunSeed {
            run_id,
            step_ids,
            ordered_steps,
            step_has_conditions,
            step_dependencies,
            step_dependency_modes,
            step_branches,
            step_collection_policies,
            step_quorum_thresholds,
            escalation_threshold,
            max_step_retries,
            max_active_nodes,
            max_active_frames,
            max_frame_depth,
        } = Self::create_run_seed_input(run_id, config)?
        else {
            unreachable!("create_run_seed_input always returns CreateRunSeed")
        };
        Ok(mob_dsl::MobMachineInput::RunFlow {
            run_id,
            step_ids,
            ordered_steps,
            step_has_conditions,
            step_dependencies,
            step_dependency_modes,
            step_branches,
            step_collection_policies,
            step_quorum_thresholds,
            escalation_threshold,
            max_step_retries,
            max_active_nodes,
            max_active_frames,
            max_frame_depth,
        })
    }

    pub(crate) fn create_frame_seed_input(
        run_id: &RunId,
        frame_id: &FrameId,
        loop_instance_id: Option<&LoopInstanceId>,
        iteration: u32,
        frame_scope: mob_dsl::FrameScope,
        spec: &FrameSpec,
        ordered: &[FlowNodeId],
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let tracked_nodes = ordered
            .iter()
            .map(|node_id| mob_dsl::FlowNodeId::from(node_id.as_str()))
            .collect();
        let ordered_nodes = ordered
            .iter()
            .map(|node_id| mob_dsl::FlowNodeId::from(node_id.as_str()))
            .collect();
        let mut node_kind = BTreeMap::new();
        let mut node_dependencies = BTreeMap::new();
        let mut node_dependency_modes = BTreeMap::new();
        let mut node_branches = BTreeMap::new();
        let mut node_step_ids = BTreeMap::new();
        let mut node_loop_ids = BTreeMap::new();

        for (node_id, node_spec) in &spec.nodes {
            let key = mob_dsl::FlowNodeId::from(node_id.as_str());
            match node_spec {
                FlowNodeSpec::Step(step) => {
                    node_kind.insert(key.clone(), mob_dsl::FlowNodeKind::Step);
                    node_dependencies.insert(
                        key.clone(),
                        step.depends_on
                            .iter()
                            .map(|dep| mob_dsl::FlowNodeId::from(dep.as_str()))
                            .collect(),
                    );
                    node_dependency_modes.insert(
                        key.clone(),
                        dependency_mode_seed_value(step.depends_on_mode.clone()),
                    );
                    node_branches.insert(
                        key.clone(),
                        step.branch
                            .as_ref()
                            .map(|branch| mob_dsl::BranchId::from(branch.as_str())),
                    );
                    node_step_ids.insert(key, mob_dsl::StepId::from(step.step_id.as_str()));
                }
                FlowNodeSpec::RepeatUntil(loop_spec) => {
                    node_kind.insert(key.clone(), mob_dsl::FlowNodeKind::Loop);
                    node_dependencies.insert(
                        key.clone(),
                        loop_spec
                            .depends_on
                            .iter()
                            .map(|dep| mob_dsl::FlowNodeId::from(dep.as_str()))
                            .collect(),
                    );
                    node_dependency_modes.insert(
                        key.clone(),
                        dependency_mode_seed_value(loop_spec.depends_on_mode.clone()),
                    );
                    node_branches.insert(key.clone(), None);
                    node_loop_ids.insert(key, mob_dsl::LoopId::from(loop_spec.loop_id.as_str()));
                }
            }
        }
        let (node_status, ready_queue) = initial_frame_node_projection(ordered, &node_dependencies);

        Ok(mob_dsl::MobMachineInput::CreateFrameSeed {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            frame_id: mob_dsl::FrameId::from(frame_id.as_str()),
            frame_scope,
            loop_instance_id: loop_instance_id.map(|id| mob_dsl::LoopInstanceId::from(id.as_str())),
            iteration,
            tracked_nodes,
            ordered_nodes,
            node_kind,
            node_dependencies,
            node_dependency_modes,
            node_branches,
            node_step_ids,
            node_loop_ids,
            node_status,
            ready_queue,
        })
    }

    pub(crate) fn create_loop_seed_input(
        snapshot: &LoopSnapshot,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        Ok(Self::create_loop_seed_input_for_start(
            &snapshot.kernel_state.loop_instance_id,
            &snapshot.kernel_state.parent_frame_id,
            &snapshot.kernel_state.parent_node_id,
            &snapshot.kernel_state.loop_id,
            snapshot.kernel_state.depth,
            snapshot.kernel_state.max_iterations,
        ))
    }

    pub(crate) fn create_loop_seed_input_for_start(
        loop_instance_id: &LoopInstanceId,
        parent_frame_id: &FrameId,
        parent_node_id: &FlowNodeId,
        loop_id: &LoopId,
        depth: u32,
        max_iterations: u32,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::CreateLoopSeed {
            loop_instance_id: mob_dsl::LoopInstanceId::from(loop_instance_id.as_str()),
            parent_frame_id: mob_dsl::FrameId::from(parent_frame_id.as_str()),
            parent_node_id: mob_dsl::FlowNodeId::from(parent_node_id.as_str()),
            loop_id: mob_dsl::LoopId::from(loop_id.as_str()),
            depth,
            max_iterations: max_iterations as u64,
        }
    }

    pub(crate) fn record_loop_body_frame_completed_input(
        loop_instance_id: &LoopInstanceId,
        iteration: u32,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted {
            loop_instance_id: mob_dsl::LoopInstanceId::from(loop_instance_id.as_str()),
            iteration: iteration as u64,
        }
    }

    pub(crate) fn record_loop_until_condition_feedback_input(
        loop_instance_id: &LoopInstanceId,
        iteration: u32,
        until_met: bool,
    ) -> mob_dsl::MobMachineInput {
        let loop_instance_id = mob_dsl::LoopInstanceId::from(loop_instance_id.as_str());
        if until_met {
            mob_dsl::MobMachineInput::RecordLoopUntilConditionMet {
                loop_instance_id,
                iteration: iteration as u64,
            }
        } else {
            mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed {
                loop_instance_id,
                iteration: iteration as u64,
            }
        }
    }

    pub fn flow_state_for_steps<I>(step_ids: I) -> Result<flow_run::State, MobError>
    where
        I: IntoIterator<Item = StepId>,
    {
        let mut steps = IndexMap::new();
        for step_id in step_ids {
            steps.insert(
                step_id,
                crate::definition::FlowStepSpec {
                    role: ProfileName::from("worker"),
                    message: meerkat_core::types::ContentInput::from("placeholder"),
                    depends_on: Vec::new(),
                    dispatch_mode: crate::definition::DispatchMode::FanOut,
                    collection_policy: crate::definition::CollectionPolicy::All,
                    condition: None,
                    timeout_ms: None,
                    expected_schema_ref: None,
                    branch: None,
                    depends_on_mode: crate::definition::DependencyMode::All,
                    allowed_tools: None,
                    blocked_tools: None,
                    output_format: crate::definition::StepOutputFormat::Json,
                },
            );
        }
        let config = FlowRunConfig {
            flow_id: FlowId::from("placeholder"),
            flow_spec: FlowSpec {
                description: None,
                steps,
                root: None,
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };
        let run_id = RunId::new();
        let seed_input = Self::create_run_seed_input(&run_id, &config)?;
        let mut authority = mob_dsl::MobMachineAuthority::new();
        authority.state.lifecycle_phase = mob_dsl::MobPhase::Running;
        mob_dsl::MobMachineMutator::apply(&mut authority, seed_input)
            .map_err(|error| MobError::Internal(format!("test CreateRunSeed rejected: {error}")))?;
        Self::flow_state_for_config_with_authority(&run_id, &config, &authority.state)
    }
}

fn mob_run_status_from_flow_phase(phase: flow_run::Phase) -> MobRunStatus {
    match phase {
        flow_run::Phase::Absent | flow_run::Phase::Pending => MobRunStatus::Pending,
        flow_run::Phase::Running => MobRunStatus::Running,
        flow_run::Phase::Completed => MobRunStatus::Completed,
        flow_run::Phase::Failed => MobRunStatus::Failed,
        flow_run::Phase::Canceled => MobRunStatus::Canceled,
    }
}

fn initial_frame_node_projection(
    ordered: &[FlowNodeId],
    node_dependencies: &BTreeMap<mob_dsl::FlowNodeId, Vec<mob_dsl::FlowNodeId>>,
) -> (
    BTreeMap<mob_dsl::FlowNodeId, mob_dsl::NodeRunStatus>,
    Vec<mob_dsl::FlowNodeId>,
) {
    let mut node_status = BTreeMap::new();
    let mut ready_queue = Vec::new();
    for node_id in ordered {
        let key = mob_dsl::FlowNodeId::from(node_id.as_str());
        let status = if node_dependencies.get(&key).is_none_or(Vec::is_empty) {
            ready_queue.push(key.clone());
            mob_dsl::NodeRunStatus::Ready
        } else {
            mob_dsl::NodeRunStatus::Pending
        };
        node_status.insert(key, status);
    }
    (node_status, ready_queue)
}

fn dependency_mode_value(mode: crate::definition::DependencyMode) -> flow_run::DependencyMode {
    match mode {
        crate::definition::DependencyMode::All => flow_run::DependencyMode::All,
        crate::definition::DependencyMode::Any => flow_run::DependencyMode::Any,
    }
}

fn dependency_mode_seed_value(mode: crate::definition::DependencyMode) -> mob_dsl::DependencyMode {
    match mode {
        crate::definition::DependencyMode::All => mob_dsl::DependencyMode::All,
        crate::definition::DependencyMode::Any => mob_dsl::DependencyMode::Any,
    }
}

fn collection_policy_kind_value(
    policy: &crate::definition::CollectionPolicy,
) -> flow_run::CollectionPolicyKind {
    match policy {
        crate::definition::CollectionPolicy::All => flow_run::CollectionPolicyKind::All,
        crate::definition::CollectionPolicy::Any => flow_run::CollectionPolicyKind::Any,
        crate::definition::CollectionPolicy::Quorum { .. } => {
            flow_run::CollectionPolicyKind::Quorum
        }
    }
}

fn collection_policy_seed_value(
    policy: &crate::definition::CollectionPolicy,
) -> mob_dsl::CollectionPolicyKind {
    match policy {
        crate::definition::CollectionPolicy::All => mob_dsl::CollectionPolicyKind::All,
        crate::definition::CollectionPolicy::Any => mob_dsl::CollectionPolicyKind::Any,
        crate::definition::CollectionPolicy::Quorum { .. } => mob_dsl::CollectionPolicyKind::Quorum,
    }
}

fn topological_steps(flow_spec: &FlowSpec) -> Result<Vec<StepId>, MobError> {
    let mut in_degree: BTreeMap<StepId, usize> = BTreeMap::new();
    let mut outgoing: BTreeMap<StepId, Vec<StepId>> = BTreeMap::new();

    for step_id in flow_spec.steps.keys() {
        in_degree.insert(step_id.clone(), 0);
        outgoing.entry(step_id.clone()).or_default();
    }

    for (step_id, step) in &flow_spec.steps {
        for dependency in &step.depends_on {
            // TLA+ NoSelfDependencyInvariant: a step cannot depend on itself.
            if dependency == step_id {
                return Err(MobError::Internal(format!(
                    "step '{step_id}' has a self-dependency"
                )));
            }
            if !in_degree.contains_key(dependency) {
                return Err(MobError::Internal(format!(
                    "step '{step_id}' depends on unknown step '{dependency}'"
                )));
            }
            *in_degree.entry(step_id.clone()).or_insert(0) += 1;
            outgoing
                .entry(dependency.clone())
                .or_default()
                .push(step_id.clone());
        }
    }

    let mut queue = VecDeque::new();
    for step_id in flow_spec.steps.keys() {
        if in_degree.get(step_id) == Some(&0) {
            queue.push_back(step_id.clone());
        }
    }

    let mut ordered = Vec::with_capacity(flow_spec.steps.len());
    while let Some(next) = queue.pop_front() {
        ordered.push(next.clone());
        if let Some(children) = outgoing.get(&next) {
            for child in children {
                if let Some(count) = in_degree.get_mut(child)
                    && *count > 0
                {
                    *count -= 1;
                    if *count == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
    }

    if ordered.len() != flow_spec.steps.len() {
        return Err(MobError::Internal(
            "flow contains a cycle; cannot compute topological order".to_string(),
        ));
    }

    Ok(ordered)
}

/// Run lifecycle states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

impl MobRunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }
}

/// Per-target step execution ledger entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLedgerEntry {
    pub step_id: StepId,
    pub agent_identity: AgentIdentity,
    pub status: StepRunStatus,
    pub output: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

/// Step execution state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRunStatus {
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

impl StepRunStatus {
    pub(crate) fn from_flow_run_status(value: &str, run_id: &RunId) -> Result<Self, MobError> {
        match value {
            "Dispatched" => Ok(Self::Dispatched),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Skipped" => Ok(Self::Skipped),
            "Canceled" => Ok(Self::Canceled),
            other => Err(MobError::Internal(format!(
                "unknown StepRunStatus variant `{other}` for {run_id}"
            ))),
        }
    }

    /// A step is terminal when it can no longer receive work dispatch or
    /// completion events. Only `Dispatched` is non-terminal.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Dispatched)
    }
}

/// Flow-level failure log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLedgerEntry {
    pub step_id: StepId,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Immutable per-run flow snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowRunConfig {
    pub flow_id: FlowId,
    pub flow_spec: FlowSpec,
    pub topology: Option<TopologySpec>,
    pub supervisor: Option<SupervisorSpec>,
    pub limits: Option<LimitsSpec>,
    pub orchestrator_role: Option<ProfileName>,
}

impl FlowRunConfig {
    pub fn from_definition(
        flow_id: FlowId,
        definition: &crate::definition::MobDefinition,
    ) -> Result<Self, MobError> {
        let flow_spec = definition
            .flows
            .get(&flow_id)
            .cloned()
            .ok_or_else(|| MobError::FlowNotFound(flow_id.clone()))?;
        let topology = definition.topology.clone();
        let orchestrator_role = definition
            .orchestrator
            .as_ref()
            .map(|orchestrator| orchestrator.profile.clone());
        if topology.is_some() && orchestrator_role.is_none() {
            return Err(MobError::Internal(
                "topology requires an orchestrator profile".to_string(),
            ));
        }
        Ok(Self {
            flow_id,
            flow_spec,
            topology,
            supervisor: definition.supervisor.clone(),
            limits: definition.limits.clone(),
            orchestrator_role,
        })
    }
}

/// Per-loop iteration output history, ordered by iteration index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LoopContextHistory {
    /// One entry per completed iteration, ordered by iteration index.
    pub iterations: Vec<IndexMap<StepId, serde_json::Value>>,
}

/// Runtime context available to condition evaluators.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowContext {
    pub run_id: RunId,
    pub activation_params: serde_json::Value,
    /// Root frame step outputs keyed by step_id.
    pub step_outputs: IndexMap<StepId, serde_json::Value>,
    /// Per-loop iteration history keyed by loop_id.
    #[serde(default)]
    pub loop_outputs: IndexMap<LoopId, LoopContextHistory>,
}

impl FlowContext {
    /// Rebuild a `FlowContext` from a persisted `MobRun` aggregate.
    pub fn from_run_aggregate(
        run: &MobRun,
        run_id: RunId,
        activation_params: serde_json::Value,
    ) -> Self {
        let loop_outputs = run
            .loop_iteration_outputs
            .iter()
            .map(|(loop_id, iterations)| {
                let history = LoopContextHistory {
                    iterations: iterations.clone(),
                };
                (loop_id.clone(), history)
            })
            .collect();

        // Seed step_outputs from root outputs, then project last-iteration
        // outputs from each completed loop into step_outputs (dogma Rule 13:
        // the projection must match what execute_frame_inner does at runtime —
        // the last iteration's body step outputs are merged into step_outputs
        // so that downstream steps/templates see them at steps.<id>).
        let mut step_outputs = run.root_step_outputs.clone();
        for iterations in run.loop_iteration_outputs.values() {
            if let Some(last_iter) = iterations.last() {
                for (sid, out) in last_iter {
                    step_outputs.insert(sid.clone(), out.clone());
                }
            }
        }

        FlowContext {
            run_id,
            activation_params,
            step_outputs,
            loop_outputs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        BackendConfig, ConditionExpr, DispatchMode, FlowStepSpec, MobDefinition,
        OrchestratorConfig, WiringRules,
    };
    use crate::ids::{BranchId, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use meerkat_core::types::ContentInput;
    use std::collections::BTreeMap;

    #[test]
    fn flow_projection_audit_requires_fail_closed_mob_machine_authority() {
        for record in flow_projection_kernel_audit() {
            assert_eq!(record.canonical_owner, "MobMachine");
            assert_eq!(
                record.role,
                FlowProjectionKernelRole::MobMachineOwnedFailClosedProjection
            );
            assert!(!record.canonical_machine);
            let allowed_inputs = [
                MobMachineCatalogInput::CreateRunSeed,
                MobMachineCatalogInput::AuthorizeFlowRunReducerCommand,
                MobMachineCatalogInput::CreateFrameSeed,
                MobMachineCatalogInput::AuthorizeFlowFrameReducerCommand,
                MobMachineCatalogInput::CreateLoopSeed,
                MobMachineCatalogInput::RecordLoopBodyFrameCompleted,
                MobMachineCatalogInput::RecordLoopUntilConditionMet,
                MobMachineCatalogInput::RecordLoopUntilConditionFailed,
                MobMachineCatalogInput::AuthorizeLoopIterationReducerCommand,
            ];
            assert!(
                record
                    .owning_inputs
                    .iter()
                    .all(|input| allowed_inputs.contains(input)),
                "projection {} must name only exact typed MobMachine authority inputs",
                record.module
            );
            assert!(
                !record.owning_inputs.is_empty(),
                "projection {} must name the MobMachine input that authorizes it",
                record.module
            );
        }
    }

    #[test]
    fn flow_reducer_apply_rejects_wrong_authority_token_family() {
        let run_state = flow_run::initial_state();
        let machine_state = mob_dsl::MobMachineState::default();
        let run_id = RunId::new();
        let frame_authority_input = mob_dsl::MobMachineInput::CreateFrameSeed {
            run_id: mob_dsl::RunId::from("run"),
            frame_id: mob_dsl::FrameId::from("frame"),
            frame_scope: crate::machines::mob_machine::FrameScope::Root,
            loop_instance_id: None,
            iteration: 0,
            tracked_nodes: Default::default(),
            ordered_nodes: Default::default(),
            node_kind: Default::default(),
            node_dependencies: Default::default(),
            node_dependency_modes: Default::default(),
            node_branches: Default::default(),
            node_step_ids: Default::default(),
            node_loop_ids: Default::default(),
            node_status: Default::default(),
            ready_queue: Default::default(),
        };
        let frame_token =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&frame_authority_input)
                .expect("frame seed input must authorize frame reducer family");
        let err = apply_mob_machine_flow_run_command(
            &run_state,
            &machine_state,
            &run_id,
            MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {}),
            frame_token,
        )
        .expect_err("flow_run reducer must reject frame authority");
        assert!(
            err.to_string().contains("cannot authorize FlowRun"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flow_reducer_apply_fails_closed_when_machine_projection_is_missing() {
        let run_state = flow_run::initial_state();
        let machine_state = mob_dsl::MobMachineState::default();
        let run_id = RunId::new();
        let step_id = StepId::from("step");
        let run_authority_input = mob_dsl::MobMachineInput::AuthorizeFlowRunReducerCommand {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            command: mob_dsl::FlowRunReducerCommandKind::CompleteStep,
            step_id: Some(mob_dsl::StepId::from(step_id.as_str())),
            run_step_key: None,
            step_status: None,
            target_count: None,
            frame_id: None,
            node_id: None,
            loop_instance_id: None,
            retry_key: None,
        };
        let run_token =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&run_authority_input)
                .expect("run command input must authorize run reducer family");
        let err = apply_mob_machine_flow_run_command(
            &run_state,
            &machine_state,
            &run_id,
            MobMachineFlowRunCommand::CompleteStep(flow_run::inputs::CompleteStep { step_id }),
            run_token,
        )
        .expect_err("machine-owned reducer authority must still require accepted projection state");
        assert!(
            err.to_string().contains("run_step_status_flat missing"),
            "unexpected error: {err}"
        );
    }

    fn sample_definition() -> MobDefinition {
        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("s1"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: ContentInput::from("do it"),
                depends_on: Vec::new(),
                dispatch_mode: DispatchMode::FanOut,
                collection_policy: crate::definition::CollectionPolicy::All,
                condition: Some(ConditionExpr::Eq {
                    path: "params.ok".to_string(),
                    value: serde_json::json!(true),
                }),
                timeout_ms: Some(2000),
                expected_schema_ref: Some("schema.json".to_string()),
                branch: Some(BranchId::from("branch-a")),
                depends_on_mode: crate::definition::DependencyMode::All,
                allowed_tools: None,
                blocked_tools: None,
                output_format: crate::definition::StepOutputFormat::Json,
            },
        );

        let mut flows = BTreeMap::new();
        flows.insert(
            FlowId::from("flow-a"),
            FlowSpec {
                description: Some("demo flow".to_string()),
                steps,
                root: None,
            },
        );

        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            ProfileBinding::Inline(Profile {
                model: "model".to_string(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "lead".to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Profile {
                model: "model".to_string(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );

        MobDefinition {
            id: MobId::from("mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows,
            topology: Some(TopologySpec {
                mode: crate::definition::PolicyMode::Advisory,
                rules: vec![crate::definition::TopologyRule {
                    from_role: ProfileName::from("lead"),
                    to_role: ProfileName::from("worker"),
                    allowed: true,
                }],
            }),
            supervisor: Some(SupervisorSpec {
                role: ProfileName::from("lead"),
                escalation_threshold: 3,
            }),
            limits: Some(LimitsSpec {
                max_flow_duration_ms: Some(60_000),
                max_step_retries: Some(1),
                max_orphaned_turns: Some(8),
                cancel_grace_timeout_ms: None,
                ..Default::default()
            }),
            spawn_policy: None,
            event_router: None,
            owner_bridge_session_id: None,
            session_cleanup_policy: crate::definition::SessionCleanupPolicy::Manual,
            is_implicit: false,
        }
    }

    #[test]
    fn test_run_status_terminal() {
        assert!(MobRunStatus::Completed.is_terminal());
        assert!(MobRunStatus::Failed.is_terminal());
        assert!(MobRunStatus::Canceled.is_terminal());
        assert!(!MobRunStatus::Pending.is_terminal());
        assert!(!MobRunStatus::Running.is_terminal());
    }

    #[test]
    fn test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a"), StepId::from("step-b")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_status = BTreeMap::from([
            (
                StepId::from("step-a"),
                Some(flow_run::StepRunStatus::Completed),
            ),
            (StepId::from("step-b"), None),
        ]);
        run.flow_state.failure_count = 3;
        run.flow_state.consecutive_failure_count = 2;
        run.flow_state.max_step_retries = 4;
        run.flow_state.escalation_threshold = 3;

        assert_eq!(
            run.ordered_steps().unwrap(),
            vec![StepId::from("step-a"), StepId::from("step-b")]
        );
        assert_eq!(
            run.step_dependencies().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), Vec::new()),
                (StepId::from("step-b"), Vec::new()),
            ])
        );
        assert_eq!(
            run.step_dependency_modes().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), DependencyMode::All),
                (StepId::from("step-b"), DependencyMode::All),
            ])
        );
        assert_eq!(
            run.step_has_conditions().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), false),
                (StepId::from("step-b"), false)
            ])
        );
        assert_eq!(
            run.step_branches().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), None),
                (StepId::from("step-b"), None)
            ])
        );
        assert_eq!(
            run.step_collection_policy_kinds().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), RunCollectionPolicyKind::All),
                (StepId::from("step-b"), RunCollectionPolicyKind::All),
            ])
        );
        assert_eq!(
            run.step_quorum_thresholds().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), 0), (StepId::from("step-b"), 0)])
        );
        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
        );
        assert_eq!(run.failure_count().unwrap(), 3);
        assert_eq!(run.consecutive_failure_count().unwrap(), 2);
        assert_eq!(run.max_step_retries().unwrap(), 4);
        assert_eq!(run.escalation_threshold().unwrap(), 3);
    }

    #[test]
    fn test_mob_run_step_status_snapshot_accepts_typed_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_status = BTreeMap::from([(
            StepId::from("step-a"),
            Some(flow_run::StepRunStatus::Completed),
        )]);
        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
        );
    }

    #[test]
    fn test_mob_run_step_status_snapshot_accepts_some_wrapped_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_status = BTreeMap::from([(
            StepId::from("step-a"),
            Some(flow_run::StepRunStatus::Completed),
        )]);

        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
        );
    }

    #[test]
    fn test_mob_run_step_dependencies_reject_invalid_dependency_entry() {
        // Typed state makes invalid dependency payloads unrepresentable.
        let run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        assert_eq!(
            run.step_dependencies().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), Vec::new())])
        );
    }

    #[test]
    fn test_mob_run_step_dependency_modes_accept_typed_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_dependency_modes =
            BTreeMap::from([(StepId::from("step-a"), flow_run::DependencyMode::All)]);

        assert_eq!(
            run.step_dependency_modes().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), DependencyMode::All)])
        );
    }

    #[test]
    fn test_mob_run_step_collection_policy_kinds_accept_typed_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_collection_policies =
            BTreeMap::from([(StepId::from("step-a"), flow_run::CollectionPolicyKind::All)]);

        assert_eq!(
            run.step_collection_policy_kinds().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), RunCollectionPolicyKind::All)])
        );
    }

    #[test]
    fn test_mob_run_step_has_conditions_rejects_non_bool_entry() {
        let run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        assert_eq!(
            run.step_has_conditions().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), false)])
        );
    }

    #[test]
    fn test_mob_run_step_branches_reject_invalid_entry() {
        let run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        assert_eq!(
            run.step_branches().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), None)])
        );
    }

    #[test]
    fn test_flow_run_config_from_definition() {
        let def = sample_definition();
        let config = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap();
        assert_eq!(config.flow_id, FlowId::from("flow-a"));
        assert_eq!(config.flow_spec.steps.len(), 1);
        assert_eq!(
            config.orchestrator_role.as_ref(),
            Some(&ProfileName::from("lead"))
        );
    }

    #[test]
    fn test_flow_run_config_from_definition_missing_flow() {
        let def = sample_definition();
        let error = FlowRunConfig::from_definition(FlowId::from("missing"), &def).unwrap_err();
        assert!(matches!(error, MobError::FlowNotFound(name) if name == "missing"));
    }

    #[test]
    fn test_flow_run_config_rejects_topology_without_orchestrator() {
        let mut def = sample_definition();
        def.orchestrator = None;
        let error = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap_err();
        assert!(
            matches!(error, MobError::Internal(message) if message.contains("topology requires")),
            "expected explicit topology/orchestrator configuration error"
        );
    }

    #[test]
    fn test_mob_run_roundtrip_json() {
        let now = Utc::now();
        let run = MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("mob"),
            flow_id: FlowId::from("flow-a"),
            status: MobRunStatus::Running,
            flow_state: MobRun::flow_state_for_steps([StepId::from("step-1")]).unwrap(),
            activation_params: serde_json::json!({"k":"v"}),
            created_at: now,
            completed_at: None,
            step_ledger: vec![StepLedgerEntry {
                step_id: StepId::from("step-1"),
                agent_identity: AgentIdentity::from("agent-1"),
                status: StepRunStatus::Completed,
                output: Some(serde_json::json!({"ok":true})),
                timestamp: now,
            }],
            failure_ledger: vec![FailureLedgerEntry {
                step_id: StepId::from("step-2"),
                reason: "boom".to_string(),
                timestamp: now,
            }],
            frames: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: BTreeMap::new(),
            flow_authority_inputs: Vec::new(),
        };

        let encoded = serde_json::to_string(&run).unwrap();
        let decoded: MobRun = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.flow_id, run.flow_id);
        assert_eq!(decoded.step_ledger.len(), 1);
        assert_eq!(decoded.failure_ledger.len(), 1);
    }

    #[test]
    fn test_flow_context_roundtrip_json() {
        let mut outputs = IndexMap::new();
        outputs.insert(StepId::from("step-1"), serde_json::json!({"a":1}));
        let context = FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({"input":"x"}),
            step_outputs: outputs,
            loop_outputs: IndexMap::new(),
        };

        let encoded = serde_json::to_string(&context).unwrap();
        let decoded: FlowContext = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.step_outputs.len(), 1);
        assert_eq!(decoded.activation_params["input"], "x");
    }

    #[test]
    fn topological_steps_rejects_self_dependency() {
        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("s1"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: ContentInput::from("do it"),
                depends_on: vec![StepId::from("s1")],
                dispatch_mode: DispatchMode::FanOut,
                collection_policy: crate::definition::CollectionPolicy::All,
                condition: None,
                timeout_ms: None,
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: crate::definition::DependencyMode::All,
                allowed_tools: None,
                blocked_tools: None,
                output_format: crate::definition::StepOutputFormat::Json,
            },
        );
        let spec = FlowSpec {
            description: None,
            steps,
            root: None,
        };
        let error = topological_steps(&spec).expect_err("self-dependency should be rejected");
        assert!(
            error.to_string().contains("self-dependency"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn step_run_status_terminal_classification() {
        assert!(StepRunStatus::Completed.is_terminal());
        assert!(StepRunStatus::Failed.is_terminal());
        assert!(StepRunStatus::Skipped.is_terminal());
        assert!(StepRunStatus::Canceled.is_terminal());
        assert!(!StepRunStatus::Dispatched.is_terminal());
    }
}
