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
use meerkat_machine_kernels::generated::mob as generated_mob;
use meerkat_machine_schema::catalog::dsl::mob_machine::MobMachineInputVariant;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub mod flow_frame;
pub mod flow_run;
pub mod loop_iteration;

pub const FLOW_RUN_PROVENANCE_AGENT_ID: &str = "__flow_system_member__";

#[must_use]
pub fn mob_run_schema_version() -> u32 {
    u32::try_from(
        mob_dsl::MobMachineAuthority::new()
            .state()
            .flow_authority_schema_version,
    )
    .expect("generated MobMachine flow authority schema version exceeds u32")
}

fn generated_record_payload<T, U>(value: &T, label: &'static str) -> Result<U, MobError>
where
    T: Serialize,
    U: DeserializeOwned,
{
    let encoded = serde_json::to_value(value).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine flow authority {label} failed generated witness encoding: {error}"
        ))
    })?;
    serde_json::from_value(encoded).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine flow authority {label} is not accepted by the generated Mob input witness: {error}"
        ))
    })
}

fn generated_flow_authority_record(
    record: &FlowAuthorityInputRecord,
) -> Result<generated_mob::Input, MobError> {
    Ok(match record {
        FlowAuthorityInputRecord::RunFlow(payload) => {
            generated_mob::Input::RunFlow(generated_record_payload(payload, "RunFlow")?)
        }
        FlowAuthorityInputRecord::CreateRunSeed(payload) => {
            generated_mob::Input::CreateRunSeed(generated_record_payload(payload, "CreateRunSeed")?)
        }
        FlowAuthorityInputRecord::AuthorizeFlowRunReducerCommand {
            run_id,
            command,
            step_id,
            step_status,
            target_count,
            frame_id,
            node_id,
            loop_instance_id,
            retry_key,
        } => generated_mob::Input::AuthorizeFlowRunReducerCommand(
            generated_mob::inputs::AuthorizeFlowRunReducerCommand {
                run_id: generated_record_payload(run_id, "AuthorizeFlowRunReducerCommand.run_id")?,
                command: generated_record_payload(
                    command,
                    "AuthorizeFlowRunReducerCommand.command",
                )?,
                step_id: generated_record_payload(
                    step_id,
                    "AuthorizeFlowRunReducerCommand.step_id",
                )?,
                step_status: generated_record_payload(
                    step_status,
                    "AuthorizeFlowRunReducerCommand.step_status",
                )?,
                target_count: *target_count,
                frame_id: generated_record_payload(
                    frame_id,
                    "AuthorizeFlowRunReducerCommand.frame_id",
                )?,
                node_id: generated_record_payload(
                    node_id,
                    "AuthorizeFlowRunReducerCommand.node_id",
                )?,
                loop_instance_id: generated_record_payload(
                    loop_instance_id,
                    "AuthorizeFlowRunReducerCommand.loop_instance_id",
                )?,
                retry_key: retry_key.clone(),
            },
        ),
        FlowAuthorityInputRecord::CreateFrameSeed(payload) => {
            generated_mob::Input::CreateFrameSeed(generated_record_payload(
                payload,
                "CreateFrameSeed",
            )?)
        }
        FlowAuthorityInputRecord::AuthorizeFlowFrameReducerCommand {
            frame_id,
            command,
            node_id,
            node_status,
            terminal_status,
        } => generated_mob::Input::AuthorizeFlowFrameReducerCommand(
            generated_mob::inputs::AuthorizeFlowFrameReducerCommand {
                frame_id: generated_record_payload(
                    frame_id,
                    "AuthorizeFlowFrameReducerCommand.frame_id",
                )?,
                command: generated_record_payload(
                    command,
                    "AuthorizeFlowFrameReducerCommand.command",
                )?,
                node_id: generated_record_payload(
                    node_id,
                    "AuthorizeFlowFrameReducerCommand.node_id",
                )?,
                node_status: generated_record_payload(
                    node_status,
                    "AuthorizeFlowFrameReducerCommand.node_status",
                )?,
                terminal_status: generated_record_payload(
                    terminal_status,
                    "AuthorizeFlowFrameReducerCommand.terminal_status",
                )?,
            },
        ),
        FlowAuthorityInputRecord::CreateLoopSeed {
            loop_instance_id,
            parent_frame_id,
            parent_node_id,
            loop_id,
            depth,
            max_iterations,
        } => generated_mob::Input::CreateLoopSeed(generated_mob::inputs::CreateLoopSeed {
            loop_instance_id: generated_record_payload(
                loop_instance_id,
                "CreateLoopSeed.loop_instance_id",
            )?,
            parent_frame_id: generated_record_payload(
                parent_frame_id,
                "CreateLoopSeed.parent_frame_id",
            )?,
            parent_node_id: generated_record_payload(
                parent_node_id,
                "CreateLoopSeed.parent_node_id",
            )?,
            loop_id: generated_record_payload(loop_id, "CreateLoopSeed.loop_id")?,
            depth: *depth,
            max_iterations: *max_iterations,
        }),
        FlowAuthorityInputRecord::RecordLoopBodyFrameCompleted {
            loop_instance_id,
            iteration,
        } => generated_mob::Input::RecordLoopBodyFrameCompleted(
            generated_mob::inputs::RecordLoopBodyFrameCompleted {
                loop_instance_id: generated_record_payload(
                    loop_instance_id,
                    "RecordLoopBodyFrameCompleted.loop_instance_id",
                )?,
                iteration: *iteration,
            },
        ),
        FlowAuthorityInputRecord::RecordLoopUntilConditionMet {
            loop_instance_id,
            iteration,
        } => generated_mob::Input::RecordLoopUntilConditionMet(
            generated_mob::inputs::RecordLoopUntilConditionMet {
                loop_instance_id: generated_record_payload(
                    loop_instance_id,
                    "RecordLoopUntilConditionMet.loop_instance_id",
                )?,
                iteration: *iteration,
            },
        ),
        FlowAuthorityInputRecord::RecordLoopUntilConditionFailed {
            loop_instance_id,
            iteration,
        } => generated_mob::Input::RecordLoopUntilConditionFailed(
            generated_mob::inputs::RecordLoopUntilConditionFailed {
                loop_instance_id: generated_record_payload(
                    loop_instance_id,
                    "RecordLoopUntilConditionFailed.loop_instance_id",
                )?,
                iteration: *iteration,
            },
        ),
        FlowAuthorityInputRecord::AuthorizeLoopIterationReducerCommand {
            loop_instance_id,
            command,
            body_frame_id,
            body_frame_iteration,
        } => generated_mob::Input::AuthorizeLoopIterationReducerCommand(
            generated_mob::inputs::AuthorizeLoopIterationReducerCommand {
                loop_instance_id: generated_record_payload(
                    loop_instance_id,
                    "AuthorizeLoopIterationReducerCommand.loop_instance_id",
                )?,
                command: generated_record_payload(
                    command,
                    "AuthorizeLoopIterationReducerCommand.command",
                )?,
                body_frame_id: generated_record_payload(
                    body_frame_id,
                    "AuthorizeLoopIterationReducerCommand.body_frame_id",
                )?,
                body_frame_iteration: *body_frame_iteration,
            },
        ),
    })
}

fn generated_flow_authority_record_variant(
    record: &FlowAuthorityInputRecord,
) -> Result<MobMachineInputVariant, MobError> {
    Ok(match generated_flow_authority_record(record)? {
        generated_mob::Input::RunFlow(_) => MobMachineInputVariant::RunFlow,
        generated_mob::Input::CreateRunSeed(_) => MobMachineInputVariant::CreateRunSeed,
        generated_mob::Input::CreateFrameSeed(_) => MobMachineInputVariant::CreateFrameSeed,
        generated_mob::Input::CreateLoopSeed(_) => MobMachineInputVariant::CreateLoopSeed,
        generated_mob::Input::RecordLoopBodyFrameCompleted(_) => {
            MobMachineInputVariant::RecordLoopBodyFrameCompleted
        }
        generated_mob::Input::RecordLoopUntilConditionMet(_) => {
            MobMachineInputVariant::RecordLoopUntilConditionMet
        }
        generated_mob::Input::RecordLoopUntilConditionFailed(_) => {
            MobMachineInputVariant::RecordLoopUntilConditionFailed
        }
        generated_mob::Input::AuthorizeFlowRunReducerCommand(_) => {
            MobMachineInputVariant::AuthorizeFlowRunReducerCommand
        }
        generated_mob::Input::AuthorizeFlowFrameReducerCommand(_) => {
            MobMachineInputVariant::AuthorizeFlowFrameReducerCommand
        }
        generated_mob::Input::AuthorizeLoopIterationReducerCommand(_) => {
            MobMachineInputVariant::AuthorizeLoopIterationReducerCommand
        }
        other => {
            return Err(MobError::Internal(format!(
                "generated Mob input witness {other:?} is not a flow authority input"
            )));
        }
    })
}

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
            | mob_dsl::MobMachineInput::ClassifyFlowRunTerminality { .. }
            | mob_dsl::MobMachineInput::ClassifyFlowRunPublicResult { .. }
            | mob_dsl::MobMachineInput::ClassifyFlowStepTerminality { .. }
            | mob_dsl::MobMachineInput::ClassifyFlowFrameTerminalStatus { .. }
            | mob_dsl::MobMachineInput::Spawn { .. }
            | mob_dsl::MobMachineInput::AuthorizeSpawnProfile { .. }
            | mob_dsl::MobMachineInput::ClassifySpawnManyFailure { .. }
            | mob_dsl::MobMachineInput::ClassifyMemberWait { .. }
            | mob_dsl::MobMachineInput::ResolveFlowDelegationEdgeAdmission { .. }
            | mob_dsl::MobMachineInput::ClassifyRemoteMemberRuntimeObservation { .. }
            | mob_dsl::MobMachineInput::ResolveSpawnMemberAdmission { .. }
            | mob_dsl::MobMachineInput::ResolveCurrentMobAdmission { .. }
            | mob_dsl::MobMachineInput::ClassifyBridgeRejectionRecovery { .. }
            | mob_dsl::MobMachineInput::EnsureMember { .. }
            | mob_dsl::MobMachineInput::Reconcile { .. }
            | mob_dsl::MobMachineInput::Retire { .. }
            | mob_dsl::MobMachineInput::RetireAbsent { .. }
            | mob_dsl::MobMachineInput::RequestPendingSessionIngressDetachForMobDestroy { .. }
            | mob_dsl::MobMachineInput::Respawn { .. }
            | mob_dsl::MobMachineInput::RetireAll
            | mob_dsl::MobMachineInput::WireMembers { .. }
            | mob_dsl::MobMachineInput::WireMembersWithTrust { .. }
            | mob_dsl::MobMachineInput::UnwireMembers { .. }
            | mob_dsl::MobMachineInput::WireExternalPeer { .. }
            | mob_dsl::MobMachineInput::RegisterMemberPeer { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberPeerRebind { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberPeerOverlay { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustWiring { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustUnwiring { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustCleanup { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustCleanupObserved { .. }
            | mob_dsl::MobMachineInput::AuthorizeExternalPeerReciprocalTrust { .. }
            | mob_dsl::MobMachineInput::UnwireExternalPeer { .. }
            | mob_dsl::MobMachineInput::ProvisionSupervisorAuthority { .. }
            | mob_dsl::MobMachineInput::ClearSupervisorPendingRotation { .. }
            | mob_dsl::MobMachineInput::RecordSupervisorPendingRotation { .. }
            | mob_dsl::MobMachineInput::CommitSupervisorRotation { .. }
            | mob_dsl::MobMachineInput::ClearSupervisorAuthorityForDestroy { .. }
            | mob_dsl::MobMachineInput::RestoreSupervisorAuthorityAfterDestroyRollback { .. }
            | mob_dsl::MobMachineInput::SubmitWork { .. }
            | mob_dsl::MobMachineInput::ResolveSubmitWorkRejection { .. }
            | mob_dsl::MobMachineInput::CancelWork { .. }
            | mob_dsl::MobMachineInput::CancelAllWork { .. }
            | mob_dsl::MobMachineInput::ResolveCancelAllWorkRejection { .. }
            | mob_dsl::MobMachineInput::Stop
            | mob_dsl::MobMachineInput::Resume
            | mob_dsl::MobMachineInput::Complete
            | mob_dsl::MobMachineInput::Reset
            | mob_dsl::MobMachineInput::Destroy
            | mob_dsl::MobMachineInput::RosterSnapshot
            | mob_dsl::MobMachineInput::ListMembers
            | mob_dsl::MobMachineInput::ListMembersIncludingRetiring
            | mob_dsl::MobMachineInput::ListAllMembers
            | mob_dsl::MobMachineInput::MemberStatus
            | mob_dsl::MobMachineInput::SubscribeAgentEvents { .. }
            | mob_dsl::MobMachineInput::SubscribeAllAgentEvents { .. }
            | mob_dsl::MobMachineInput::SubscribeMobEvents { .. }
            | mob_dsl::MobMachineInput::SubscribeStructuralEvents { .. }
            | mob_dsl::MobMachineInput::AuthorizeMobEventRouterMemberSubscription { .. }
            | mob_dsl::MobMachineInput::AuthorizeMobEventRouterMemberRemoval { .. }
            | mob_dsl::MobMachineInput::PollEvents
            | mob_dsl::MobMachineInput::PollEventsStrict { .. }
            | mob_dsl::MobMachineInput::ReplayAllEvents
            | mob_dsl::MobMachineInput::RecordOperatorActionProvenance { .. }
            | mob_dsl::MobMachineInput::GetMember
            | mob_dsl::MobMachineInput::SetSpawnPolicy { .. }
            | mob_dsl::MobMachineInput::ResolveSpawnPolicy { .. }
            | mob_dsl::MobMachineInput::BindOwnerBridgeSession { .. }
            | mob_dsl::MobMachineInput::Shutdown
            | mob_dsl::MobMachineInput::ForceCancel { .. }
            | mob_dsl::MobMachineInput::KickoffMarkPending { .. }
            | mob_dsl::MobMachineInput::KickoffMarkStarting { .. }
            | mob_dsl::MobMachineInput::StartupMarkReady { .. }
            | mob_dsl::MobMachineInput::KickoffResolveStarted { .. }
            | mob_dsl::MobMachineInput::KickoffResolveCallbackPending { .. }
            | mob_dsl::MobMachineInput::KickoffResolveFailed { .. }
            | mob_dsl::MobMachineInput::KickoffCancelRequested { .. }
            | mob_dsl::MobMachineInput::KickoffClear { .. }
            | mob_dsl::MobMachineInput::RecordCoordinationWorkIntent { .. }
            | mob_dsl::MobMachineInput::RecordCoordinationResourceClaim { .. }
            | mob_dsl::MobMachineInput::UpdateCoordinationWorkIntentStatus { .. }
            | mob_dsl::MobMachineInput::UpdateCoordinationResourceClaimStatus { .. }
            | mob_dsl::MobMachineInput::ObserveCoordinationResourceClaimOverlap { .. }
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
    #[serde(default)]
    pub step_status: BTreeMap<mob_dsl::StepId, Option<mob_dsl::StepRunStatus>>,
    #[serde(default)]
    pub output_recorded: BTreeMap<mob_dsl::StepId, bool>,
    #[serde(default)]
    pub step_condition_results: BTreeMap<mob_dsl::StepId, Option<bool>>,
    pub step_has_conditions: BTreeMap<mob_dsl::StepId, bool>,
    pub step_dependencies: BTreeMap<mob_dsl::StepId, Vec<mob_dsl::StepId>>,
    pub step_dependency_modes: BTreeMap<mob_dsl::StepId, mob_dsl::DependencyMode>,
    pub step_branches: BTreeMap<mob_dsl::StepId, Option<mob_dsl::BranchId>>,
    pub step_collection_policies: BTreeMap<mob_dsl::StepId, mob_dsl::CollectionPolicyKind>,
    pub step_quorum_thresholds: BTreeMap<mob_dsl::StepId, u32>,
    #[serde(default)]
    pub step_target_counts: BTreeMap<mob_dsl::StepId, u64>,
    #[serde(default)]
    pub step_target_success_counts: BTreeMap<mob_dsl::StepId, u64>,
    #[serde(default)]
    pub step_target_terminal_failure_counts: BTreeMap<mob_dsl::StepId, u64>,
    pub escalation_threshold: u64,
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
    #[serde(default)]
    pub output_recorded: BTreeMap<mob_dsl::FlowNodeId, bool>,
    #[serde(default)]
    pub node_condition_results: BTreeMap<mob_dsl::FlowNodeId, Option<bool>>,
    #[serde(default)]
    pub last_admitted_node: Option<mob_dsl::FlowNodeId>,
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
                step_status,
                output_recorded,
                step_condition_results,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                step_target_counts,
                step_target_success_counts,
                step_target_terminal_failure_counts,
                escalation_threshold,
                max_step_retries,
                max_active_nodes,
                max_active_frames,
                max_frame_depth,
            } => Self::RunFlow(FlowRunSeedAuthorityRecord {
                run_id,
                step_ids,
                ordered_steps,
                step_status,
                output_recorded,
                step_condition_results,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                step_target_counts,
                step_target_success_counts,
                step_target_terminal_failure_counts,
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
                step_status,
                output_recorded,
                step_condition_results,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                step_target_counts,
                step_target_success_counts,
                step_target_terminal_failure_counts,
                escalation_threshold,
                max_step_retries,
                max_active_nodes,
                max_active_frames,
                max_frame_depth,
            } => Self::CreateRunSeed(FlowRunSeedAuthorityRecord {
                run_id,
                step_ids,
                ordered_steps,
                step_status,
                output_recorded,
                step_condition_results,
                step_has_conditions,
                step_dependencies,
                step_dependency_modes,
                step_branches,
                step_collection_policies,
                step_quorum_thresholds,
                step_target_counts,
                step_target_success_counts,
                step_target_terminal_failure_counts,
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
                output_recorded,
                node_condition_results,
                last_admitted_node,
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
                output_recorded,
                node_condition_results,
                last_admitted_node,
            }),
            mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command,
                node_id,
                node_status,
                terminal_status,
            } => Self::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command: command.into(),
                node_id,
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
            | mob_dsl::MobMachineInput::ClassifyFlowRunTerminality { .. }
            | mob_dsl::MobMachineInput::ClassifyFlowRunPublicResult { .. }
            | mob_dsl::MobMachineInput::ClassifyFlowStepTerminality { .. }
            | mob_dsl::MobMachineInput::ClassifyFlowFrameTerminalStatus { .. }
            | mob_dsl::MobMachineInput::Spawn { .. }
            | mob_dsl::MobMachineInput::AuthorizeSpawnProfile { .. }
            | mob_dsl::MobMachineInput::ClassifySpawnManyFailure { .. }
            | mob_dsl::MobMachineInput::ClassifyMemberWait { .. }
            | mob_dsl::MobMachineInput::ResolveFlowDelegationEdgeAdmission { .. }
            | mob_dsl::MobMachineInput::ClassifyRemoteMemberRuntimeObservation { .. }
            | mob_dsl::MobMachineInput::ResolveSpawnMemberAdmission { .. }
            | mob_dsl::MobMachineInput::ResolveCurrentMobAdmission { .. }
            | mob_dsl::MobMachineInput::ClassifyBridgeRejectionRecovery { .. }
            | mob_dsl::MobMachineInput::EnsureMember { .. }
            | mob_dsl::MobMachineInput::Reconcile { .. }
            | mob_dsl::MobMachineInput::Retire { .. }
            | mob_dsl::MobMachineInput::RetireAbsent { .. }
            | mob_dsl::MobMachineInput::RequestPendingSessionIngressDetachForMobDestroy {
                ..
            }
            | mob_dsl::MobMachineInput::Respawn { .. }
            | mob_dsl::MobMachineInput::RetireAll
            | mob_dsl::MobMachineInput::WireMembers { .. }
            | mob_dsl::MobMachineInput::WireMembersWithTrust { .. }
            | mob_dsl::MobMachineInput::UnwireMembers { .. }
            | mob_dsl::MobMachineInput::WireExternalPeer { .. }
            | mob_dsl::MobMachineInput::RegisterMemberPeer { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberPeerRebind { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberPeerOverlay { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustWiring { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustUnwiring { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustCleanup { .. }
            | mob_dsl::MobMachineInput::AuthorizeMemberTrustCleanupObserved { .. }
            | mob_dsl::MobMachineInput::AuthorizeExternalPeerReciprocalTrust { .. }
            | mob_dsl::MobMachineInput::UnwireExternalPeer { .. }
            | mob_dsl::MobMachineInput::ProvisionSupervisorAuthority { .. }
            | mob_dsl::MobMachineInput::ClearSupervisorPendingRotation { .. }
            | mob_dsl::MobMachineInput::RecordSupervisorPendingRotation { .. }
            | mob_dsl::MobMachineInput::CommitSupervisorRotation { .. }
            | mob_dsl::MobMachineInput::ClearSupervisorAuthorityForDestroy { .. }
            | mob_dsl::MobMachineInput::RestoreSupervisorAuthorityAfterDestroyRollback { .. }
            | mob_dsl::MobMachineInput::SubmitWork { .. }
            | mob_dsl::MobMachineInput::ResolveSubmitWorkRejection { .. }
            | mob_dsl::MobMachineInput::CancelWork { .. }
            | mob_dsl::MobMachineInput::CancelAllWork { .. }
            | mob_dsl::MobMachineInput::ResolveCancelAllWorkRejection { .. }
            | mob_dsl::MobMachineInput::Stop
            | mob_dsl::MobMachineInput::Resume
            | mob_dsl::MobMachineInput::Complete
            | mob_dsl::MobMachineInput::Reset
            | mob_dsl::MobMachineInput::Destroy
            | mob_dsl::MobMachineInput::RosterSnapshot
            | mob_dsl::MobMachineInput::ListMembers
            | mob_dsl::MobMachineInput::ListMembersIncludingRetiring
            | mob_dsl::MobMachineInput::ListAllMembers
            | mob_dsl::MobMachineInput::MemberStatus
            | mob_dsl::MobMachineInput::SubscribeAgentEvents { .. }
            | mob_dsl::MobMachineInput::SubscribeAllAgentEvents { .. }
            | mob_dsl::MobMachineInput::SubscribeMobEvents { .. }
            | mob_dsl::MobMachineInput::SubscribeStructuralEvents { .. }
            | mob_dsl::MobMachineInput::AuthorizeMobEventRouterMemberSubscription { .. }
            | mob_dsl::MobMachineInput::AuthorizeMobEventRouterMemberRemoval { .. }
            | mob_dsl::MobMachineInput::PollEvents
            | mob_dsl::MobMachineInput::PollEventsStrict { .. }
            | mob_dsl::MobMachineInput::ReplayAllEvents
            | mob_dsl::MobMachineInput::RecordOperatorActionProvenance { .. }
            | mob_dsl::MobMachineInput::GetMember
            | mob_dsl::MobMachineInput::SetSpawnPolicy { .. }
            | mob_dsl::MobMachineInput::ResolveSpawnPolicy { .. }
            | mob_dsl::MobMachineInput::BindOwnerBridgeSession { .. }
            | mob_dsl::MobMachineInput::Shutdown
            | mob_dsl::MobMachineInput::ForceCancel { .. }
            | mob_dsl::MobMachineInput::KickoffMarkPending { .. }
            | mob_dsl::MobMachineInput::KickoffMarkStarting { .. }
            | mob_dsl::MobMachineInput::StartupMarkReady { .. }
            | mob_dsl::MobMachineInput::KickoffResolveStarted { .. }
            | mob_dsl::MobMachineInput::KickoffResolveCallbackPending { .. }
            | mob_dsl::MobMachineInput::KickoffResolveFailed { .. }
            | mob_dsl::MobMachineInput::KickoffCancelRequested { .. }
            | mob_dsl::MobMachineInput::KickoffClear { .. }
            | mob_dsl::MobMachineInput::RecordCoordinationWorkIntent { .. }
            | mob_dsl::MobMachineInput::RecordCoordinationResourceClaim { .. }
            | mob_dsl::MobMachineInput::UpdateCoordinationWorkIntentStatus { .. }
            | mob_dsl::MobMachineInput::UpdateCoordinationResourceClaimStatus { .. }
            | mob_dsl::MobMachineInput::ObserveCoordinationResourceClaimOverlap { .. } => {
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
        let input_variant = generated_flow_authority_record_variant(&record)?;
        if !canonical_flow_authority_input_variant_manifest().contains(&input_variant) {
            return Err(MobError::Internal(format!(
                "MobMachine input variant {input_variant:?} is not in the generated flow authority manifest"
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
                step_status: record.step_status,
                output_recorded: record.output_recorded,
                step_condition_results: record.step_condition_results,
                step_has_conditions: record.step_has_conditions,
                step_dependencies: record.step_dependencies,
                step_dependency_modes: record.step_dependency_modes,
                step_branches: record.step_branches,
                step_collection_policies: record.step_collection_policies,
                step_quorum_thresholds: record.step_quorum_thresholds,
                step_target_counts: record.step_target_counts,
                step_target_success_counts: record.step_target_success_counts,
                step_target_terminal_failure_counts: record.step_target_terminal_failure_counts,
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
                step_status: record.step_status,
                output_recorded: record.output_recorded,
                step_condition_results: record.step_condition_results,
                step_has_conditions: record.step_has_conditions,
                step_dependencies: record.step_dependencies,
                step_dependency_modes: record.step_dependency_modes,
                step_branches: record.step_branches,
                step_collection_policies: record.step_collection_policies,
                step_quorum_thresholds: record.step_quorum_thresholds,
                step_target_counts: record.step_target_counts,
                step_target_success_counts: record.step_target_success_counts,
                step_target_terminal_failure_counts: record.step_target_terminal_failure_counts,
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
                output_recorded: record.output_recorded,
                node_condition_results: record.node_condition_results,
                last_admitted_node: record.last_admitted_node,
            },
            Self::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command,
                node_id,
                node_status,
                terminal_status,
            } => mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand {
                frame_id,
                command: command.into(),
                node_id,
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
    machine_effects: &[mob_dsl::MobMachineEffect],
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
            machine_effects,
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
                machine_effects,
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
        required_machine_value(&machine_state.run_step_status, &run_key, "run_step_status")?
            .clone(),
    );
    state.output_recorded = project_step_map(
        required_machine_value(
            &machine_state.run_output_recorded,
            &run_key,
            "run_output_recorded",
        )?
        .clone(),
        |v| v,
    );
    state.step_condition_results = project_step_map(
        required_machine_value(
            &machine_state.run_step_condition_results,
            &run_key,
            "run_step_condition_results",
        )?
        .clone(),
        |v| v,
    );
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
        required_machine_value(
            &machine_state.run_step_target_counts,
            &run_key,
            "run_step_target_counts",
        )?
        .clone(),
        saturating_u64_to_u32,
    );
    state.step_target_success_counts = project_step_map(
        required_machine_value(
            &machine_state.run_step_target_success_counts,
            &run_key,
            "run_step_target_success_counts",
        )?
        .clone(),
        saturating_u64_to_u32,
    );
    state.step_target_terminal_failure_counts = project_step_map(
        required_machine_value(
            &machine_state.run_step_target_terminal_failure_counts,
            &run_key,
            "run_step_target_terminal_failure_counts",
        )?
        .clone(),
        saturating_u64_to_u32,
    );
    state.target_retry_counts = machine_state
        .run_target_retry_counts
        .get(&run_key)
        .cloned()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_target_retry_counts missing key {run_key:?}"
            ))
        })?
        .into_iter()
        .map(|(key, value)| (key, saturating_u64_to_u32(value)))
        .collect();
    project_flow_run_counters_from_machine(&mut state, machine_state, &run_key)?;
    state.escalation_threshold = required_machine_value(
        &machine_state.run_escalation_threshold,
        &run_key,
        "run_escalation_threshold",
    )
    .map(|value| saturating_u64_to_u32(*value))?;
    state.max_step_retries = *required_machine_value(
        &machine_state.run_max_step_retries,
        &run_key,
        "run_max_step_retries",
    )?;
    state.ready_frames = machine_state
        .run_ready_frames
        .get(&run_key)
        .cloned()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_ready_frames missing key {run_key:?}"
            ))
        })?
        .iter()
        .map(project_frame_id)
        .collect();
    state.ready_frame_membership = machine_state
        .run_ready_frame_membership
        .get(&run_key)
        .cloned()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_ready_frame_membership missing key {run_key:?}"
            ))
        })?
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
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_pending_body_frame_loops missing key {run_key:?}"
            ))
        })?
        .iter()
        .map(project_loop_instance_id)
        .collect();
    let mut pending_body_frame_loop_membership: BTreeSet<LoopInstanceId> = machine_state
        .run_pending_body_frame_loop_membership
        .get(&run_key)
        .cloned()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_pending_body_frame_loop_membership missing key {run_key:?}"
            ))
        })?
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
    if let Some(frame_id) = required_machine_value(
        &machine_state.run_last_granted_frame,
        &run_key,
        "run_last_granted_frame",
    )? {
        state.last_granted_frame = project_frame_id(frame_id);
    }
    if let Some(loop_instance_id) = required_machine_value(
        &machine_state.run_last_granted_loop,
        &run_key,
        "run_last_granted_loop",
    )? {
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

    validate_flow_run_projection_facts(&state)?;

    Ok(state)
}

fn validate_flow_run_projection_facts(state: &flow_run::State) -> Result<(), MobError> {
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_status.keys(),
        "run_step_status",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.output_recorded.keys(),
        "run_output_recorded",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_condition_results.keys(),
        "run_step_condition_results",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_dependencies.keys(),
        "run_step_dependencies",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_dependency_modes.keys(),
        "run_step_dependency_modes",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_branches.keys(),
        "run_step_branches",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_collection_policies.keys(),
        "run_step_collection_policies",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_quorum_thresholds.keys(),
        "run_step_quorum_thresholds",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_target_counts.keys(),
        "run_step_target_counts",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_target_success_counts.keys(),
        "run_step_target_success_counts",
    )?;
    validate_step_projection_keys(
        &state.phase,
        &state.tracked_steps,
        state.step_target_terminal_failure_counts.keys(),
        "run_step_target_terminal_failure_counts",
    )?;
    Ok(())
}

fn validate_step_projection_keys<'a>(
    phase: &flow_run::Phase,
    tracked_steps: &BTreeSet<StepId>,
    keys: impl Iterator<Item = &'a StepId>,
    field: &'static str,
) -> Result<(), MobError> {
    let keys = keys.cloned().collect::<BTreeSet<_>>();
    for step_id in tracked_steps {
        if !keys.contains(step_id) {
            return Err(MobError::Internal(format!(
                "MobMachine run projection field {field} missing tracked step '{step_id}' in phase {phase:?}"
            )));
        }
    }
    for step_id in &keys {
        if !tracked_steps.contains(step_id) {
            return Err(MobError::Internal(format!(
                "MobMachine run projection field {field} contains untracked step '{step_id}' in phase {phase:?}"
            )));
        }
    }
    Ok(())
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
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    let status = required_machine_value(
        required_machine_value(&machine_state.run_step_status, &run_key, "run_step_status")?,
        &step_key,
        "run_step_status",
    )?;
    let Some(status) = status else {
        return Err(MobError::Internal(format!(
            "MobMachine run_step_status missing accepted projection for run '{run_id}' step '{step_id}'"
        )));
    };
    let projected_status = project_step_run_status(*status);
    if projected_status != expected_status {
        return Err(MobError::Internal(format!(
            "MobMachine run_step_status projected {projected_status:?} for run '{run_id}' step '{step_id}', expected {expected_status:?}"
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
    machine_effects: &[mob_dsl::MobMachineEffect],
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
    append_generated_flow_run_effects(&mut outcome.effects, machine_effects, step_id);
    Ok(outcome)
}

fn project_flow_run_frame_step_status_from_machine(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    frame_id: &FrameId,
    node_id: &FlowNodeId,
    machine_effects: &[mob_dsl::MobMachineEffect],
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
            append_generated_flow_run_effects(&mut outcome.effects, machine_effects, step_id);
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

fn append_generated_flow_run_effects(
    effects: &mut Vec<flow_run::Effect>,
    machine_effects: &[mob_dsl::MobMachineEffect],
    step_id: &StepId,
) {
    for effect in machine_effects {
        match effect {
            mob_dsl::MobMachineEffect::AppendFailureLedger => {
                effects.push(flow_run::Effect::AppendFailureLedger(
                    flow_run::effects::AppendFailureLedger {
                        step_id: step_id.clone(),
                    },
                ));
            }
            mob_dsl::MobMachineEffect::EscalateSupervisor => {
                effects.push(flow_run::Effect::EscalateSupervisor(
                    flow_run::effects::EscalateSupervisor {
                        step_id: step_id.clone(),
                    },
                ));
            }
            _ => {}
        }
    }
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
    let recorded = projected_run_step_value(
        &machine_state.run_output_recorded,
        run_id,
        step_id,
        "run_output_recorded",
    )?;
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
    let projected = machine_state
        .run_step_condition_results
        .get(&run_key)
        .and_then(|conditions| conditions.get(&step_key))
        .copied()
        .flatten()
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
    let count = machine_state
        .run_target_retry_counts
        .get(&run_key)
        .and_then(|counts| counts.get(retry_key))
        .copied()
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
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_ready_frames missing key {run_key:?}"
            ))
        })?
        .iter()
        .map(project_frame_id)
        .collect::<Vec<_>>();
    let mut ready_frame_membership = machine_state
        .run_ready_frame_membership
        .get(&run_key)
        .cloned()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_ready_frame_membership missing key {run_key:?}"
            ))
        })?
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
    let machine_last_granted_frame = required_machine_value(
        &machine_state.run_last_granted_frame,
        &run_key,
        "run_last_granted_frame",
    )?;
    if let Some(last_granted_frame) =
        last_granted_frame.or_else(|| machine_last_granted_frame.as_ref().map(project_frame_id))
    {
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
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_pending_body_frame_loops missing key {run_key:?}"
            ))
        })?
        .iter()
        .map(project_loop_instance_id)
        .collect::<Vec<_>>();
    let mut pending_body_frame_loop_membership = machine_state
        .run_pending_body_frame_loop_membership
        .get(&run_key)
        .cloned()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine projection field run_pending_body_frame_loop_membership missing key {run_key:?}"
            ))
        })?
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
    let machine_last_granted_loop = required_machine_value(
        &machine_state.run_last_granted_loop,
        &run_key,
        "run_last_granted_loop",
    )?;
    if let Some(last_granted_loop) = last_granted_loop.or_else(|| {
        machine_last_granted_loop
            .as_ref()
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
        .and_then(|frame_id| frame_id.as_ref().map(project_frame_id))
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
        .and_then(|loop_instance_id| loop_instance_id.as_ref().map(project_loop_instance_id))
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
    map: &BTreeMap<mob_dsl::RunId, BTreeMap<mob_dsl::StepId, V>>,
    run_id: &RunId,
    step_id: &StepId,
    field: &'static str,
) -> Result<V, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    map.get(&run_key)
        .and_then(|values| values.get(&step_key))
        .copied()
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
    let frame_parent_loop = required_machine_value(
        &machine_state.frame_parent_loop,
        &frame_key,
        "frame_parent_loop",
    )?;
    let loop_instance_id = match (frame_scope, frame_parent_loop) {
        (flow_frame::FrameScope::Root, None) => LoopInstanceId::from(String::new()),
        (flow_frame::FrameScope::Root, Some(parent)) => {
            return Err(MobError::Internal(format!(
                "MobMachine root frame '{frame_id}' unexpectedly projected parent loop '{}'",
                parent.0
            )));
        }
        (flow_frame::FrameScope::Body, Some(parent)) => project_loop_instance_id(parent),
        (flow_frame::FrameScope::Body, None) => {
            return Err(MobError::Internal(format!(
                "MobMachine body frame '{frame_id}' missing generated parent loop"
            )));
        }
    };
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
    let frame_node_status = required_machine_value(
        &machine_state.frame_node_status,
        &frame_key,
        "frame_node_status",
    )?;
    let frame_ready_queue = required_machine_value(
        &machine_state.frame_ready_queue,
        &frame_key,
        "frame_ready_queue",
    )?;
    let frame_output_recorded = required_machine_value(
        &machine_state.frame_output_recorded,
        &frame_key,
        "frame_output_recorded",
    )?;
    let frame_node_condition_results = required_machine_value(
        &machine_state.frame_node_condition_results,
        &frame_key,
        "frame_node_condition_results",
    )?;
    let frame_last_admitted_node = required_machine_value(
        &machine_state.frame_last_admitted_node,
        &frame_key,
        "frame_last_admitted_node",
    )?;
    let state = flow_frame::State {
        phase,
        frame_id: frame_id.clone(),
        frame_scope,
        loop_instance_id,
        iteration,
        last_admitted_node: frame_last_admitted_node
            .as_ref()
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
        node_status: project_node_map(frame_node_status.clone(), project_node_run_status),
        ready_queue: frame_ready_queue.iter().map(project_flow_node_id).collect(),
        output_recorded: project_node_map(frame_output_recorded.clone(), |v| v),
        node_condition_results: project_node_map(frame_node_condition_results.clone(), |v| v),
    };
    validate_flow_frame_projection_facts(&state)?;
    Ok(state)
}

fn validate_flow_frame_projection_facts(state: &flow_frame::State) -> Result<(), MobError> {
    for node_id in &state.tracked_nodes {
        if !state.node_status.contains_key(node_id) {
            return Err(MobError::Internal(format!(
                "MobMachine frame '{}' missing generated node status for tracked node '{}'",
                state.frame_id, node_id
            )));
        }
    }
    for node_id in state.node_status.keys() {
        if !state.tracked_nodes.contains(node_id) {
            return Err(MobError::Internal(format!(
                "MobMachine frame '{}' projected status for untracked node '{}'",
                state.frame_id, node_id
            )));
        }
    }
    let ready_members = state
        .ready_queue
        .iter()
        .cloned()
        .collect::<BTreeSet<FlowNodeId>>();
    for node_id in &state.ready_queue {
        match state.node_status.get(node_id) {
            Some(flow_frame::NodeRunStatus::Ready) => {}
            Some(other) => {
                return Err(MobError::Internal(format!(
                    "MobMachine frame '{}' ready queue contains node '{}' with status {:?}",
                    state.frame_id, node_id, other
                )));
            }
            None => {
                return Err(MobError::Internal(format!(
                    "MobMachine frame '{}' ready queue contains node '{}' without generated status",
                    state.frame_id, node_id
                )));
            }
        }
    }
    for (node_id, status) in &state.node_status {
        if *status == flow_frame::NodeRunStatus::Ready && !ready_members.contains(node_id) {
            return Err(MobError::Internal(format!(
                "MobMachine frame '{}' generated ready node '{}' missing from ready queue",
                state.frame_id, node_id
            )));
        }
    }
    validate_node_projection_keys(state, state.output_recorded.keys(), "frame_output_recorded")?;
    validate_node_projection_keys(
        state,
        state.node_condition_results.keys(),
        "frame_node_condition_results",
    )?;
    Ok(())
}

fn validate_node_projection_keys<'a>(
    state: &flow_frame::State,
    keys: impl Iterator<Item = &'a FlowNodeId>,
    field: &'static str,
) -> Result<(), MobError> {
    let keys = keys.cloned().collect::<BTreeSet<_>>();
    for node_id in &state.tracked_nodes {
        if !keys.contains(node_id) {
            return Err(MobError::Internal(format!(
                "MobMachine frame '{}' projection field {field} missing tracked node '{}'",
                state.frame_id, node_id
            )));
        }
    }
    for node_id in &keys {
        if !state.tracked_nodes.contains(node_id) {
            return Err(MobError::Internal(format!(
                "MobMachine frame '{}' projection field {field} contains untracked node '{}'",
                state.frame_id, node_id
            )));
        }
    }
    Ok(())
}

pub(crate) fn project_flow_frame_authority_state_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
) -> Result<flow_frame::State, MobError> {
    let state = project_flow_frame_state_from_machine(machine_state, frame_id, BTreeSet::new())?;
    let branch_winners = completed_branch_winners(&state);
    project_flow_frame_state_from_machine(machine_state, frame_id, branch_winners)
}

fn completed_branch_winners(state: &flow_frame::State) -> BTreeSet<BranchId> {
    state
        .node_status
        .iter()
        .filter(|(_, status)| **status == flow_frame::NodeRunStatus::Completed)
        .filter_map(|(node_id, _)| state.node_branches.get(node_id).and_then(Clone::clone))
        .collect()
}

fn refresh_completed_branch_winners(mut state: flow_frame::State) -> flow_frame::State {
    state.branch_winners = completed_branch_winners(&state);
    state
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
    let admitted_node = if next_state.last_admitted_node.as_str().is_empty() {
        return Err(MobError::Internal(format!(
            "MobMachine frame '{}' missing generated admitted-node witness",
            state.frame_id
        )));
    } else {
        next_state.last_admitted_node.clone()
    };
    if next_state.node_status.get(&admitted_node) != Some(&flow_frame::NodeRunStatus::Running) {
        return Err(MobError::Internal(format!(
            "MobMachine frame '{}' admitted node '{}' was not projected as Running",
            state.frame_id, admitted_node
        )));
    }
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
    let next_state = refresh_completed_branch_winners(project_flow_frame_state_from_machine(
        machine_state,
        &state.frame_id,
        state.branch_winners.clone(),
    )?);
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
    let next_state = refresh_completed_branch_winners(project_flow_frame_state_from_machine(
        machine_state,
        &state.frame_id,
        state.branch_winners.clone(),
    )?);
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

fn mob_run_status_to_machine(status: &MobRunStatus) -> mob_dsl::FlowRunStatus {
    match status {
        MobRunStatus::Pending => mob_dsl::FlowRunStatus::Pending,
        MobRunStatus::Running => mob_dsl::FlowRunStatus::Running,
        MobRunStatus::Completed => mob_dsl::FlowRunStatus::Completed,
        MobRunStatus::Failed => mob_dsl::FlowRunStatus::Failed,
        MobRunStatus::Canceled => mob_dsl::FlowRunStatus::Canceled,
    }
}

fn step_run_status_to_machine(status: &StepRunStatus) -> mob_dsl::StepRunStatus {
    match status {
        StepRunStatus::Dispatched => mob_dsl::StepRunStatus::Dispatched,
        StepRunStatus::Completed => mob_dsl::StepRunStatus::Completed,
        StepRunStatus::Failed => mob_dsl::StepRunStatus::Failed,
        StepRunStatus::Skipped => mob_dsl::StepRunStatus::Skipped,
        StepRunStatus::Canceled => mob_dsl::StepRunStatus::Canceled,
    }
}

fn classify_mob_machine_stateless(
    input: mob_dsl::MobMachineInput,
    label: &'static str,
) -> Result<Vec<mob_dsl::MobMachineEffect>, MobError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    let input_debug = format!("{input:?}");
    mob_dsl::MobMachineMutator::apply(&mut authority, input)
        .map_err(|error| {
            MobError::Internal(format!(
                "MobMachine stateless classifier ({label}) rejected {input_debug}: {error}"
            ))
        })
        .map(|transition| transition.into_effects())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobFlowRunPublicResultClass {
    Success,
    Error,
}

impl From<mob_dsl::FlowRunPublicResultClassKind> for MobFlowRunPublicResultClass {
    fn from(value: mob_dsl::FlowRunPublicResultClassKind) -> Self {
        match value {
            mob_dsl::FlowRunPublicResultClassKind::Success => Self::Success,
            mob_dsl::FlowRunPublicResultClassKind::Failure => Self::Error,
        }
    }
}

pub fn mob_machine_run_status_is_terminal(
    run_id: &RunId,
    status: &MobRunStatus,
) -> Result<bool, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let effects = classify_mob_machine_stateless(
        mob_dsl::MobMachineInput::ClassifyFlowRunTerminality {
            run_id: run_key.clone(),
            status: mob_run_status_to_machine(status),
        },
        "ClassifyFlowRunTerminality",
    )?;
    let mut terminal = None;
    for effect in effects {
        match effect {
            mob_dsl::MobMachineEffect::FlowRunTerminal { run_id } if run_id == run_key => {
                terminal = Some(true);
            }
            mob_dsl::MobMachineEffect::FlowRunNonTerminal { run_id } if run_id == run_key => {
                terminal = Some(false);
            }
            other => {
                return Err(MobError::Internal(format!(
                    "MobMachine run terminality classifier emitted unexpected effect: {other:?}"
                )));
            }
        }
    }
    terminal.ok_or_else(|| {
        MobError::Internal(format!(
            "MobMachine run terminality classifier emitted no effect for run '{run_id}'"
        ))
    })
}

pub fn mob_machine_step_status_is_terminal(
    run_id: &RunId,
    step_id: &StepId,
    status: &StepRunStatus,
) -> Result<bool, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    let effects = classify_mob_machine_stateless(
        mob_dsl::MobMachineInput::ClassifyFlowStepTerminality {
            run_id: run_key.clone(),
            step_id: step_key.clone(),
            status: step_run_status_to_machine(status),
        },
        "ClassifyFlowStepTerminality",
    )?;
    let mut terminal = None;
    for effect in effects {
        match effect {
            mob_dsl::MobMachineEffect::FlowStepTerminal { run_id, step_id }
                if run_id == run_key && step_id == step_key =>
            {
                terminal = Some(true);
            }
            mob_dsl::MobMachineEffect::FlowStepNonTerminal { run_id, step_id }
                if run_id == run_key && step_id == step_key =>
            {
                terminal = Some(false);
            }
            other => {
                return Err(MobError::Internal(format!(
                    "MobMachine step terminality classifier emitted unexpected effect: {other:?}"
                )));
            }
        }
    }
    terminal.ok_or_else(|| {
        MobError::Internal(format!(
            "MobMachine step terminality classifier emitted no effect for run '{run_id}' step '{step_id}'"
        ))
    })
}

pub fn mob_machine_run_public_result_class(
    run_id: &RunId,
    status: &MobRunStatus,
) -> Result<MobFlowRunPublicResultClass, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let effects = classify_mob_machine_stateless(
        mob_dsl::MobMachineInput::ClassifyFlowRunPublicResult {
            run_id: run_key.clone(),
            status: mob_run_status_to_machine(status),
        },
        "ClassifyFlowRunPublicResult",
    )?;
    let mut result = None;
    for effect in effects {
        match effect {
            mob_dsl::MobMachineEffect::FlowRunPublicResultClassified {
                run_id,
                result: classified,
            } if run_id == run_key => {
                result = Some(MobFlowRunPublicResultClass::from(classified));
            }
            other => {
                return Err(MobError::Internal(format!(
                    "MobMachine flow public-result classifier emitted unexpected effect: {other:?}"
                )));
            }
        }
    }
    result.ok_or_else(|| {
        MobError::Internal(format!(
            "MobMachine flow public-result classifier emitted no effect for run '{run_id}'"
        ))
    })
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
    pub fn public_flow_status_run_value(run: Option<&Self>) -> Result<serde_json::Value, MobError> {
        match run {
            Some(run) => run.public_status_value(),
            None => Ok(serde_json::Value::Null),
        }
    }

    /// Public flow-status projection. Store-local timestamps are deliberately
    /// omitted because MobMachine does not own them as semantic flow facts.
    pub fn public_status_value(&self) -> Result<serde_json::Value, MobError> {
        let mut value = serde_json::to_value(self).map_err(|error| {
            MobError::Internal(format!(
                "public flow status projection failed to encode run '{}': {error}",
                self.run_id
            ))
        })?;
        let Some(object) = value.as_object_mut() else {
            return Err(MobError::Internal(format!(
                "public flow status projection for run '{}' did not encode as an object",
                self.run_id
            )));
        };

        object.remove("created_at");
        object.remove("completed_at");

        for field in ["step_ledger", "failure_ledger"] {
            let Some(entries) = object.get_mut(field) else {
                continue;
            };
            let Some(entries) = entries.as_array_mut() else {
                return Err(MobError::Internal(format!(
                    "public flow status projection for run '{}' encoded {field} as non-array",
                    self.run_id
                )));
            };
            for entry in entries {
                let Some(entry) = entry.as_object_mut() else {
                    return Err(MobError::Internal(format!(
                        "public flow status projection for run '{}' encoded {field} entry as non-object",
                        self.run_id
                    )));
                };
                entry.remove("timestamp");
            }
        }

        Ok(value)
    }

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
            schema_version: mob_run_schema_version(),
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
        if records.is_empty() {
            return Err(MobError::Internal(format!(
                "run '{}' store mutation missing MobMachine authority input",
                self.run_id
            )));
        }
        self.flow_authority_inputs.extend(records);
        Ok(())
    }

    pub(crate) fn replay_flow_authority_inputs_into(
        authority: &mut mob_dsl::MobMachineAuthority,
        inputs: &[FlowAuthorityInputRecord],
        context: &str,
    ) -> Result<(), MobError> {
        for record in inputs {
            let input_variant = generated_flow_authority_record_variant(record)?;
            if !canonical_flow_authority_input_variant_manifest().contains(&input_variant) {
                return Err(MobError::Internal(format!(
                    "persisted MobMachine input variant {input_variant:?} is not in the generated flow authority manifest"
                )));
            }
            let input = record.to_machine_input();
            let input_debug = format!("{input:?}");
            let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(
                |error| {
                    MobError::Internal(format!(
                        "MobMachine flow authority replay ({context}) rejected {input_debug}: {error}"
                    ))
                },
            )?;
            let _ = transition;
        }
        Ok(())
    }

    fn validate_flow_authority_projection_core(&self) -> Result<(), MobError> {
        let expected_schema_version = mob_run_schema_version();
        if self.schema_version != expected_schema_version {
            return Err(MobError::Internal(format!(
                "flow authority log projection mismatch for run '{}': schema_version {} != {}",
                self.run_id, self.schema_version, expected_schema_version
            )));
        }
        if self.flow_authority_inputs.is_empty() {
            return Err(MobError::Internal(format!(
                "flow authority log projection mismatch for run '{}': missing MobMachine authority inputs",
                self.run_id
            )));
        }

        let mut authority = mob_dsl::MobMachineAuthority::new();
        Self::replay_flow_authority_inputs_into(
            &mut authority,
            &self.flow_authority_inputs,
            "recovery_validate_run_projection",
        )?;

        let projected_flow_state =
            project_flow_run_state_from_machine(authority.state(), &self.run_id)?;
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
            .state()
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
                project_flow_frame_authority_state_from_machine(authority.state(), frame_id)?;
            if projected != snapshot.kernel_state {
                return Err(MobError::Internal(format!(
                    "flow authority log projection mismatch for run '{}': frame '{}' diverged",
                    self.run_id, frame_id
                )));
            }
        }

        let projected_loop_ids = authority
            .state()
            .loop_phase
            .keys()
            .filter(|loop_instance_id| {
                authority
                    .state()
                    .loop_parent_frame
                    .get(*loop_instance_id)
                    .and_then(|frame_id| authority.state().frame_run.get(frame_id))
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
                project_loop_iteration_state_from_machine(authority.state(), loop_instance_id)?;
            if projected != snapshot.kernel_state {
                return Err(MobError::Internal(format!(
                    "flow authority log projection mismatch for run '{}': loop '{}' diverged",
                    self.run_id, loop_instance_id
                )));
            }
        }

        Ok(())
    }

    pub(crate) fn validate_flow_authority_projection(&self) -> Result<(), MobError> {
        self.validate_flow_authority_projection_core()?;
        self.validate_provenance_ledgers()
    }

    fn validate_provenance_ledgers(&self) -> Result<(), MobError> {
        let reducer_authorities = self
            .flow_authority_inputs
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                matches!(
                    record,
                    FlowAuthorityInputRecord::AuthorizeFlowRunReducerCommand { .. }
                )
            })
            .map(|(index, record)| {
                (
                    index,
                    MobRunProvenanceAuthority {
                        input: record.to_machine_input(),
                    },
                )
            })
            .collect::<Vec<_>>();

        let mut used_step_authorities = BTreeSet::new();
        for entry in &self.step_ledger {
            let Some((authority_index, _)) =
                reducer_authorities
                    .iter()
                    .find(|(authority_index, authority)| {
                        !used_step_authorities.contains(authority_index)
                            && authority.validate_step_entry(self, entry).is_ok()
                    })
            else {
                return Err(MobError::Internal(format!(
                    "run '{}' step ledger entry for step '{}' status {:?} is not authorized by the MobMachine authority log",
                    self.run_id, entry.step_id, entry.status
                )));
            };
            used_step_authorities.insert(*authority_index);
        }

        let mut used_failure_authorities = BTreeSet::new();
        for entry in &self.failure_ledger {
            let Some((authority_index, _)) =
                reducer_authorities
                    .iter()
                    .find(|(authority_index, authority)| {
                        !used_failure_authorities.contains(authority_index)
                            && authority.validate_failure_entry(self, entry).is_ok()
                    })
            else {
                return Err(MobError::Internal(format!(
                    "run '{}' failure ledger entry for step '{}' is not authorized by the MobMachine authority log",
                    self.run_id, entry.step_id
                )));
            };
            used_failure_authorities.insert(*authority_index);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn flow_state_for_config(config: &FlowRunConfig) -> Result<flow_run::State, MobError> {
        let run_id = RunId::new();
        let seed_input = Self::create_run_seed_input(&run_id, config)?;
        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(&mut authority, seed_input)
            .map_err(|error| MobError::Internal(format!("test CreateRunSeed rejected: {error}")))?;
        Self::flow_state_for_config_with_authority(&run_id, config, authority.state())
    }

    #[cfg(test)]
    pub(crate) fn authority_backed_for_steps<I>(
        run_id: RunId,
        mob_id: MobId,
        flow_id: FlowId,
        step_ids: I,
        status: MobRunStatus,
        activation_params: serde_json::Value,
    ) -> Result<Self, MobError>
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
            flow_id: flow_id.clone(),
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

        let seed_input = Self::create_run_seed_input(&run_id, &config)?;
        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(&mut authority, seed_input.clone())
            .map_err(|error| MobError::Internal(format!("test CreateRunSeed rejected: {error}")))?;
        let flow_state =
            Self::flow_state_for_config_with_authority(&run_id, &config, authority.state())?;
        let mut run =
            Self::pending_with_run_id(run_id, mob_id, flow_id, flow_state, activation_params);
        run.append_flow_authority_inputs(vec![seed_input])?;

        if status != MobRunStatus::Pending {
            let start_input = run.apply_flow_run_command_for_test(
                &mut authority,
                MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {}),
            )?;
            run.status = MobRunStatus::Running;
            run.append_flow_authority_inputs(vec![start_input])?;
        }

        let terminal_command = match status {
            MobRunStatus::Pending | MobRunStatus::Running => None,
            MobRunStatus::Completed => Some(MobMachineFlowRunCommand::TerminalizeCompleted(
                flow_run::inputs::TerminalizeCompleted {},
            )),
            MobRunStatus::Failed => Some(MobMachineFlowRunCommand::TerminalizeFailed(
                flow_run::inputs::TerminalizeFailed {},
            )),
            MobRunStatus::Canceled => Some(MobMachineFlowRunCommand::TerminalizeCanceled(
                flow_run::inputs::TerminalizeCanceled {},
            )),
        };
        if let Some(command) = terminal_command {
            let terminal_input = run.apply_flow_run_command_for_test(&mut authority, command)?;
            run.status = status;
            run.completed_at = Some(Utc::now());
            run.append_flow_authority_inputs(vec![terminal_input])?;
        }

        run.validate_flow_authority_projection()?;
        Ok(run)
    }

    #[cfg(test)]
    pub(crate) fn flow_run_command_projection_for_test(
        &self,
        command: MobMachineFlowRunCommand,
    ) -> Result<(flow_run::State, mob_dsl::MobMachineInput), MobError> {
        let mut authority = mob_dsl::MobMachineAuthority::new();
        Self::replay_flow_authority_inputs_into(
            &mut authority,
            &self.flow_authority_inputs,
            "test_project_flow_run_command",
        )?;
        let mut run = self.clone();
        let input = run.apply_flow_run_command_for_test(&mut authority, command)?;
        Ok((run.flow_state, input))
    }

    #[cfg(test)]
    fn apply_flow_run_command_for_test(
        &mut self,
        authority: &mut mob_dsl::MobMachineAuthority,
        command: MobMachineFlowRunCommand,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let input = command.authority_input(&self.run_id);
        let transition =
            mob_dsl::MobMachineMutator::apply(authority, input.clone()).map_err(|error| {
                MobError::Internal(format!("test flow run authority input rejected: {error}"))
            })?;
        let authority_token =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&input)?;
        let outcome = apply_mob_machine_flow_run_command(
            &self.flow_state,
            authority.state(),
            &self.run_id,
            command,
            authority_token,
            transition.effects(),
        )?;
        self.flow_state = outcome.next_state;
        Ok(input)
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
        let step_ids = config
            .flow_spec
            .steps
            .keys()
            .map(|step_id| mob_dsl::StepId::from(step_id.as_str()))
            .collect::<BTreeSet<_>>();
        let step_status = step_ids
            .iter()
            .cloned()
            .map(|step_id| (step_id, None))
            .collect();
        let output_recorded = step_ids
            .iter()
            .cloned()
            .map(|step_id| (step_id, false))
            .collect();
        let step_condition_results = step_ids
            .iter()
            .cloned()
            .map(|step_id| (step_id, None))
            .collect();
        let step_target_counts = step_ids
            .iter()
            .cloned()
            .map(|step_id| (step_id, 0))
            .collect();
        let step_target_success_counts = step_ids
            .iter()
            .cloned()
            .map(|step_id| (step_id, 0))
            .collect();
        let step_target_terminal_failure_counts = step_ids
            .iter()
            .cloned()
            .map(|step_id| (step_id, 0))
            .collect();
        Ok(mob_dsl::MobMachineInput::CreateRunSeed {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            step_ids,
            ordered_steps: ordered_steps
                .iter()
                .map(|step_id| mob_dsl::StepId::from(step_id.as_str()))
                .collect(),
            step_status,
            output_recorded,
            step_condition_results,
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
            step_target_counts,
            step_target_success_counts,
            step_target_terminal_failure_counts,
            escalation_threshold: config
                .supervisor
                .as_ref()
                .map_or(0, |supervisor| u64::from(supervisor.escalation_threshold)),
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
            step_status,
            output_recorded,
            step_condition_results,
            step_has_conditions,
            step_dependencies,
            step_dependency_modes,
            step_branches,
            step_collection_policies,
            step_quorum_thresholds,
            step_target_counts,
            step_target_success_counts,
            step_target_terminal_failure_counts,
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
            step_status,
            output_recorded,
            step_condition_results,
            step_has_conditions,
            step_dependencies,
            step_dependency_modes,
            step_branches,
            step_collection_policies,
            step_quorum_thresholds,
            step_target_counts,
            step_target_success_counts,
            step_target_terminal_failure_counts,
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
        let tracked_nodes: BTreeSet<mob_dsl::FlowNodeId> = ordered
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
        let output_recorded = tracked_nodes
            .iter()
            .cloned()
            .map(|node_id| (node_id, false))
            .collect();
        let node_condition_results = tracked_nodes
            .iter()
            .cloned()
            .map(|node_id| (node_id, None))
            .collect();

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
            output_recorded,
            node_condition_results,
            last_admitted_node: None,
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
        mob_dsl::MobMachineMutator::apply(&mut authority, seed_input)
            .map_err(|error| MobError::Internal(format!("test CreateRunSeed rejected: {error}")))?;
        Self::flow_state_for_config_with_authority(&run_id, &config, authority.state())
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
}

/// Flow-level failure log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLedgerEntry {
    pub step_id: StepId,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_report: Option<meerkat_core::event::AgentErrorReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<meerkat_core::event::TurnErrorMetadata>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MobRunProvenanceAuthority {
    input: mob_dsl::MobMachineInput,
}

impl MobRunProvenanceAuthority {
    pub fn from_flow_authority_input(input: mob_dsl::MobMachineInput) -> Result<Self, MobError> {
        let record = FlowAuthorityInputRecord::from_machine_input(input.clone())?;
        match record {
            FlowAuthorityInputRecord::AuthorizeFlowRunReducerCommand { .. } => Ok(Self { input }),
            _ => Err(MobError::Internal(format!(
                "MobRun provenance authority requires a flow-run reducer authority input, got {input:?}"
            ))),
        }
    }

    fn record(&self) -> Result<FlowAuthorityInputRecord, MobError> {
        FlowAuthorityInputRecord::from_machine_input(self.input.clone())
    }

    fn validate_present(&self, run: &MobRun) -> Result<FlowAuthorityInputRecord, MobError> {
        let record = self.record()?;
        if run
            .flow_authority_inputs
            .iter()
            .any(|existing| existing == &record)
        {
            Ok(record)
        } else {
            Err(MobError::Internal(format!(
                "run '{}' provenance authority input is not present in the MobMachine authority log",
                run.run_id
            )))
        }
    }

    pub(crate) fn validate_step_entry(
        &self,
        run: &MobRun,
        entry: &StepLedgerEntry,
    ) -> Result<(), MobError> {
        run.validate_flow_authority_projection_core()?;
        if entry.agent_identity.as_str() != FLOW_RUN_PROVENANCE_AGENT_ID {
            return Err(MobError::Internal(format!(
                "run '{}' step ledger authority only permits system provenance '{}', entry has '{}'",
                run.run_id, FLOW_RUN_PROVENANCE_AGENT_ID, entry.agent_identity
            )));
        }
        let record = self.validate_present(run)?;
        let FlowAuthorityInputRecord::AuthorizeFlowRunReducerCommand {
            command, step_id, ..
        } = record
        else {
            unreachable!("validate_present returned a flow-run reducer authority record")
        };
        let step_id = step_id
            .map(|step_id| project_step_id(&step_id))
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "run '{}' step ledger authority {:?} did not name a step",
                    run.run_id, command
                ))
            })?;
        if step_id != entry.step_id {
            return Err(MobError::Internal(format!(
                "run '{}' step ledger authority step '{}' does not match entry step '{}'",
                run.run_id, step_id, entry.step_id
            )));
        }

        let expected = match command {
            FlowRunReducerCommandRecord::DispatchStep => Some(StepRunStatus::Dispatched),
            FlowRunReducerCommandRecord::CompleteStep => Some(StepRunStatus::Completed),
            FlowRunReducerCommandRecord::FailStep => Some(StepRunStatus::Failed),
            FlowRunReducerCommandRecord::SkipStep => Some(StepRunStatus::Skipped),
            FlowRunReducerCommandRecord::CancelStep => Some(StepRunStatus::Canceled),
            FlowRunReducerCommandRecord::ProjectFrameStepStatus => {
                run.step_status_snapshot()?.get(&step_id).cloned()
            }
            _ => None,
        }
        .ok_or_else(|| {
            MobError::Internal(format!(
                "run '{}' authority command {:?} cannot authorize a step ledger entry",
                run.run_id, command
            ))
        })?;

        if expected != entry.status {
            return Err(MobError::Internal(format!(
                "run '{}' step ledger authority projected {:?} for step '{}', entry has {:?}",
                run.run_id, expected, entry.step_id, entry.status
            )));
        }
        let expected_output = match command {
            FlowRunReducerCommandRecord::ProjectFrameStepStatus
                if expected == StepRunStatus::Completed =>
            {
                projected_step_output(run, &entry.step_id)
            }
            _ => None,
        };
        if entry.output.as_ref() != expected_output {
            return Err(MobError::Internal(format!(
                "run '{}' step ledger authority projected output {:?} for step '{}', entry has {:?}",
                run.run_id, expected_output, entry.step_id, entry.output
            )));
        }
        Ok(())
    }

    pub(crate) fn validate_failure_entry(
        &self,
        run: &MobRun,
        entry: &FailureLedgerEntry,
    ) -> Result<(), MobError> {
        if entry.error_report.is_some() || entry.error.is_some() {
            return Err(MobError::Internal(format!(
                "run '{}' failure ledger terminal error metadata requires generated mob authority",
                run.run_id
            )));
        }
        run.validate_flow_authority_projection_core()?;
        let record = self.validate_present(run)?;
        let FlowAuthorityInputRecord::AuthorizeFlowRunReducerCommand {
            command, step_id, ..
        } = record
        else {
            unreachable!("validate_present returned a flow-run reducer authority record")
        };
        let step_id = step_id
            .map(|step_id| project_step_id(&step_id))
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "run '{}' failure ledger authority {:?} did not name a step",
                    run.run_id, command
                ))
            })?;
        if step_id != entry.step_id {
            return Err(MobError::Internal(format!(
                "run '{}' failure ledger authority step '{}' does not match entry step '{}'",
                run.run_id, step_id, entry.step_id
            )));
        }

        let can_append_failure = match command {
            FlowRunReducerCommandRecord::FailStep => true,
            FlowRunReducerCommandRecord::ProjectFrameStepStatus => {
                run.step_status_snapshot()?.get(&step_id) == Some(&StepRunStatus::Failed)
            }
            _ => false,
        };
        if !can_append_failure {
            return Err(MobError::Internal(format!(
                "run '{}' authority command {:?} cannot authorize a failure ledger entry",
                run.run_id, command
            )));
        }
        Ok(())
    }
}

fn projected_step_output<'a>(run: &'a MobRun, step_id: &StepId) -> Option<&'a serde_json::Value> {
    let mut output = run.root_step_outputs.get(step_id);
    for iterations in run.loop_iteration_outputs.values() {
        for iteration in iterations {
            if let Some(value) = iteration.get(step_id) {
                output = Some(value);
            }
        }
    }
    output
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
    fn flow_authority_journal_records_require_generated_mob_input_witness() {
        let def = sample_definition();
        let config = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def)
            .expect("sample flow config");
        let run_id = RunId::new();
        let seed_input = MobRun::create_run_seed_input(&run_id, &config)
            .expect("generated MobMachine seed input");
        let record = FlowAuthorityInputRecord::from_machine_input(seed_input)
            .expect("seed input should produce a journal record");

        assert_eq!(
            generated_flow_authority_record_variant(&record).expect("generated witness"),
            MobMachineInputVariant::CreateRunSeed
        );
        assert!(
            matches!(
                generated_flow_authority_record(&record).expect("generated Mob input witness"),
                generated_mob::Input::CreateRunSeed(_)
            ),
            "flow authority journal records must be accepted by generated Mob input structs"
        );
    }

    #[test]
    fn mob_run_schema_version_is_owned_by_mob_machine_state() {
        let machine_version = mob_dsl::MobMachineAuthority::new()
            .state()
            .flow_authority_schema_version;
        assert_eq!(
            mob_run_schema_version(),
            u32::try_from(machine_version).expect("test schema version fits u32")
        );
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
            output_recorded: Default::default(),
            node_condition_results: Default::default(),
            last_admitted_node: None,
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
            &[],
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
            &[],
        )
        .expect_err("machine-owned reducer authority must still require accepted projection state");
        assert!(
            err.to_string().contains("run_step_status"),
            "unexpected error: {err}"
        );
    }

    fn machine_state_with_flow_frame_projection(
        node_status: mob_dsl::NodeRunStatus,
        ready_queue: Vec<mob_dsl::FlowNodeId>,
    ) -> (mob_dsl::MobMachineState, FrameId, mob_dsl::FrameId) {
        let mut machine_state = mob_dsl::MobMachineState::default();
        let frame_id = FrameId::from("frame");
        let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
        let node_key = mob_dsl::FlowNodeId::from("node");
        let mut tracked_nodes = BTreeSet::new();
        tracked_nodes.insert(node_key.clone());
        let ordered_nodes = vec![node_key.clone()];

        machine_state
            .frame_phase
            .insert(frame_key.clone(), mob_dsl::FrameStatus::Running);
        machine_state
            .frame_scope
            .insert(frame_key.clone(), mob_dsl::FrameScope::Root);
        machine_state
            .frame_parent_loop
            .insert(frame_key.clone(), None);
        machine_state.frame_iteration.insert(frame_key.clone(), 0);
        machine_state
            .frame_tracked_nodes
            .insert(frame_key.clone(), tracked_nodes);
        machine_state
            .frame_ordered_nodes
            .insert(frame_key.clone(), ordered_nodes);
        machine_state.frame_node_kind.insert(
            frame_key.clone(),
            BTreeMap::from([(node_key.clone(), mob_dsl::FlowNodeKind::Step)]),
        );
        machine_state.frame_node_dependencies.insert(
            frame_key.clone(),
            BTreeMap::from([(node_key.clone(), Vec::new())]),
        );
        machine_state.frame_node_dependency_modes.insert(
            frame_key.clone(),
            BTreeMap::from([(node_key.clone(), mob_dsl::DependencyMode::All)]),
        );
        machine_state.frame_node_branches.insert(
            frame_key.clone(),
            BTreeMap::from([(node_key.clone(), None)]),
        );
        machine_state.frame_node_status.insert(
            frame_key.clone(),
            BTreeMap::from([(node_key.clone(), node_status)]),
        );
        machine_state
            .frame_ready_queue
            .insert(frame_key.clone(), ready_queue);
        machine_state.frame_output_recorded.insert(
            frame_key.clone(),
            BTreeMap::from([(node_key.clone(), false)]),
        );
        machine_state
            .frame_node_condition_results
            .insert(frame_key.clone(), BTreeMap::from([(node_key, None)]));
        machine_state
            .frame_last_admitted_node
            .insert(frame_key.clone(), None);

        (machine_state, frame_id, frame_key)
    }

    #[test]
    fn flow_frame_projection_fails_closed_without_generated_node_status() {
        let ready_node = mob_dsl::FlowNodeId::from("node");
        let (mut machine_state, frame_id, frame_key) = machine_state_with_flow_frame_projection(
            mob_dsl::NodeRunStatus::Ready,
            vec![ready_node],
        );
        machine_state.frame_node_status.remove(&frame_key);

        let err = project_flow_frame_state_from_machine(&machine_state, &frame_id, BTreeSet::new())
            .expect_err("projection must require generated frame_node_status");
        assert!(
            err.to_string().contains("frame_node_status"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flow_frame_projection_does_not_synthesize_ready_status_from_queue() {
        let ready_node = mob_dsl::FlowNodeId::from("node");
        let (machine_state, frame_id, _) = machine_state_with_flow_frame_projection(
            mob_dsl::NodeRunStatus::Pending,
            vec![ready_node],
        );

        let err = project_flow_frame_state_from_machine(&machine_state, &frame_id, BTreeSet::new())
            .expect_err("projection must not promote pending nodes to ready");
        assert!(
            err.to_string().contains("ready queue contains node"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flow_frame_projection_fails_closed_for_body_without_parent_loop() {
        let ready_node = mob_dsl::FlowNodeId::from("node");
        let (mut machine_state, frame_id, frame_key) = machine_state_with_flow_frame_projection(
            mob_dsl::NodeRunStatus::Ready,
            vec![ready_node],
        );
        machine_state
            .frame_scope
            .insert(frame_key, mob_dsl::FrameScope::Body);

        let err = project_flow_frame_state_from_machine(&machine_state, &frame_id, BTreeSet::new())
            .expect_err("body frame projection must require generated parent loop identity");
        assert!(
            err.to_string().contains("missing generated parent loop"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flow_frame_admit_requires_generated_admitted_node_witness() {
        let ready_node = mob_dsl::FlowNodeId::from("node");
        let (machine_state, frame_id, _) = machine_state_with_flow_frame_projection(
            mob_dsl::NodeRunStatus::Ready,
            vec![ready_node],
        );
        let previous =
            project_flow_frame_state_from_machine(&machine_state, &frame_id, BTreeSet::new())
                .expect("fixture should project ready frame state");
        let node = mob_dsl::FlowNodeId::from("node");
        let (mut machine_state, _, frame_key) =
            machine_state_with_flow_frame_projection(mob_dsl::NodeRunStatus::Running, Vec::new());
        machine_state
            .frame_last_admitted_node
            .insert(frame_key, None);

        let err = project_flow_frame_admit_from_machine(&previous, &machine_state)
            .expect_err("admit projection must require generated admitted-node witness");
        assert!(
            err.to_string()
                .contains("missing generated admitted-node witness"),
            "unexpected error for node {node:?}: {err}"
        );
    }

    #[test]
    fn flow_run_projection_fails_closed_without_generated_seed_defaults() {
        let def = sample_definition();
        let config = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap();
        let run_id = RunId::new();
        let input = MobRun::create_run_seed_input(&run_id, &config).unwrap();
        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(&mut authority, input)
            .expect("CreateRunSeed fixture should be accepted");
        let mut machine_state = authority.state().clone();
        let run_key = mob_dsl::RunId::from(run_id.to_string());
        machine_state.run_output_recorded.remove(&run_key);

        let err = project_flow_run_state_from_machine(&machine_state, &run_id)
            .expect_err("run projection must require generated output defaults");
        assert!(
            err.to_string().contains("run_output_recorded"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn create_run_seed_rejects_extra_default_map_keys() {
        let def = sample_definition();
        let config = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap();
        let run_id = RunId::new();
        let mut input = MobRun::create_run_seed_input(&run_id, &config).unwrap();
        let mob_dsl::MobMachineInput::CreateRunSeed { step_status, .. } = &mut input else {
            panic!("create_run_seed_input always returns CreateRunSeed");
        };
        step_status.insert(mob_dsl::StepId::from("untracked"), None);

        let mut authority = mob_dsl::MobMachineAuthority::new();
        let err = mob_dsl::MobMachineMutator::apply(&mut authority, input)
            .expect_err("CreateRunSeed must reject untracked default-map keys");
        assert!(
            err.to_string().contains("guard rejected"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn create_frame_seed_rejects_extra_default_map_keys() {
        let def = sample_definition();
        let config = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap();
        let run_id = RunId::new();
        let run_seed = MobRun::create_run_seed_input(&run_id, &config).unwrap();
        let frame_id = FrameId::from("frame");
        let frame_spec = FrameSpec {
            nodes: IndexMap::new(),
        };
        let mut frame_seed = MobRun::create_frame_seed_input(
            &run_id,
            &frame_id,
            None,
            0,
            mob_dsl::FrameScope::Root,
            &frame_spec,
            &[],
        )
        .unwrap();
        let mob_dsl::MobMachineInput::CreateFrameSeed {
            output_recorded, ..
        } = &mut frame_seed
        else {
            panic!("create_frame_seed_input always returns CreateFrameSeed");
        };
        output_recorded.insert(mob_dsl::FlowNodeId::from("untracked"), false);

        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(&mut authority, run_seed)
            .expect("run seed should be accepted");
        let err = mob_dsl::MobMachineMutator::apply(&mut authority, frame_seed)
            .expect_err("CreateFrameSeed must reject untracked default-map keys");
        assert!(
            err.to_string().contains("guard rejected"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn schema6_flow_authority_records_decode_but_fail_version_gate() {
        let run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            FlowId::from("flow-a"),
            [StepId::from("s1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run fixture");
        let mut encoded = serde_json::to_value(&run).expect("serialize run");
        encoded["schema_version"] = serde_json::json!(6);
        if let Some(payload) = encoded["flow_authority_inputs"]
            .get_mut(0)
            .and_then(|record| record.get_mut("payload"))
            .and_then(serde_json::Value::as_object_mut)
        {
            payload.remove("step_status");
            payload.remove("output_recorded");
            payload.remove("step_condition_results");
            payload.remove("step_target_counts");
            payload.remove("step_target_success_counts");
            payload.remove("step_target_terminal_failure_counts");
        }

        let decoded: MobRun =
            serde_json::from_value(encoded).expect("schema-6 authority records should decode");
        let err = decoded
            .validate_flow_authority_projection()
            .expect_err("schema-6 authority log must fail the version gate");
        assert!(
            err.to_string().contains("schema_version 6 != 7"),
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

        let mut definition = MobDefinition::explicit("mob");
        definition.orchestrator = Some(OrchestratorConfig {
            profile: ProfileName::from("lead"),
        });
        definition.profiles = profiles;
        definition.flows = flows;
        definition.topology = Some(TopologySpec {
            mode: crate::definition::PolicyMode::Advisory,
            rules: vec![crate::definition::TopologyRule {
                from_role: ProfileName::from("lead"),
                to_role: ProfileName::from("worker"),
                allowed: true,
            }],
        });
        definition.supervisor = Some(SupervisorSpec {
            role: ProfileName::from("lead"),
            escalation_threshold: 3,
        });
        definition.limits = Some(LimitsSpec {
            max_flow_duration_ms: Some(60_000),
            max_step_retries: Some(1),
            max_orphaned_turns: Some(8),
            cancel_grace_timeout_ms: None,
            ..Default::default()
        });
        definition
    }

    #[test]
    fn test_run_status_terminal() {
        let run_id = RunId::new();
        assert!(mob_machine_run_status_is_terminal(&run_id, &MobRunStatus::Completed).unwrap());
        assert!(mob_machine_run_status_is_terminal(&run_id, &MobRunStatus::Failed).unwrap());
        assert!(mob_machine_run_status_is_terminal(&run_id, &MobRunStatus::Canceled).unwrap());
        assert!(!mob_machine_run_status_is_terminal(&run_id, &MobRunStatus::Pending).unwrap());
        assert!(!mob_machine_run_status_is_terminal(&run_id, &MobRunStatus::Running).unwrap());
    }

    #[test]
    fn test_run_public_result_class() {
        let run_id = RunId::new();
        assert_eq!(
            mob_machine_run_public_result_class(&run_id, &MobRunStatus::Completed).unwrap(),
            MobFlowRunPublicResultClass::Success
        );
        assert_eq!(
            mob_machine_run_public_result_class(&run_id, &MobRunStatus::Failed).unwrap(),
            MobFlowRunPublicResultClass::Error
        );
        assert_eq!(
            mob_machine_run_public_result_class(&run_id, &MobRunStatus::Canceled).unwrap(),
            MobFlowRunPublicResultClass::Error
        );
        assert!(mob_machine_run_public_result_class(&run_id, &MobRunStatus::Pending).is_err());
        assert!(mob_machine_run_public_result_class(&run_id, &MobRunStatus::Running).is_err());
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
                error_report: None,
                error: None,
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
    fn public_flow_status_projection_omits_store_local_timestamps() {
        let now = Utc::now();
        let run = MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("mob"),
            flow_id: FlowId::from("flow-a"),
            status: MobRunStatus::Completed,
            flow_state: MobRun::flow_state_for_steps([StepId::from("step-1")]).unwrap(),
            activation_params: serde_json::json!({"k":"v"}),
            created_at: now,
            completed_at: Some(now),
            step_ledger: vec![StepLedgerEntry {
                step_id: StepId::from("step-1"),
                agent_identity: AgentIdentity::from(FLOW_RUN_PROVENANCE_AGENT_ID),
                status: StepRunStatus::Completed,
                output: Some(serde_json::json!({"ok":true})),
                timestamp: now,
            }],
            failure_ledger: vec![FailureLedgerEntry {
                step_id: StepId::from("step-1"),
                reason: "boom".to_string(),
                error_report: None,
                error: None,
                timestamp: now,
            }],
            frames: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: mob_run_schema_version(),
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: BTreeMap::new(),
            flow_authority_inputs: Vec::new(),
        };

        let value = run.public_status_value().expect("public projection");
        let object = value.as_object().expect("run projection is object");
        assert!(!object.contains_key("created_at"));
        assert!(!object.contains_key("completed_at"));
        assert_eq!(object.get("status"), Some(&serde_json::json!("completed")));
        assert!(object.contains_key("flow_state"));

        let step_entry = object["step_ledger"]
            .as_array()
            .expect("step_ledger array")
            .first()
            .expect("step ledger entry")
            .as_object()
            .expect("step ledger entry object");
        assert!(!step_entry.contains_key("timestamp"));
        assert_eq!(
            step_entry.get("status"),
            Some(&serde_json::json!("completed"))
        );

        let failure_entry = object["failure_ledger"]
            .as_array()
            .expect("failure_ledger array")
            .first()
            .expect("failure ledger entry")
            .as_object()
            .expect("failure ledger entry object");
        assert!(!failure_entry.contains_key("timestamp"));
        assert_eq!(
            failure_entry.get("reason"),
            Some(&serde_json::json!("boom"))
        );
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
        let run_id = RunId::new();
        let step_id = StepId::from("step");
        assert!(
            mob_machine_step_status_is_terminal(&run_id, &step_id, &StepRunStatus::Completed)
                .unwrap()
        );
        assert!(
            mob_machine_step_status_is_terminal(&run_id, &step_id, &StepRunStatus::Failed).unwrap()
        );
        assert!(
            mob_machine_step_status_is_terminal(&run_id, &step_id, &StepRunStatus::Skipped)
                .unwrap()
        );
        assert!(
            mob_machine_step_status_is_terminal(&run_id, &step_id, &StepRunStatus::Canceled)
                .unwrap()
        );
        assert!(
            !mob_machine_step_status_is_terminal(&run_id, &step_id, &StepRunStatus::Dispatched)
                .unwrap()
        );
    }
}
