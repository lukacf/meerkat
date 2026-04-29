//! DSL-generated machine schemas.
//!
//! These modules contain `machine!` invocations that generate the same
//! `MachineSchema` values as the hand-written catalog entries. They use
//! `rust: "self"` so the generated `schema()` function references
//! `crate::MachineSchema` instead of `meerkat_machine_schema::MachineSchema`.
#![allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::cmp_owned,
    clippy::assign_op_pattern
)]

/// Extension trait providing `.get()` on Option to support the `option_value`
/// schema pattern (`Expr::MapGet { map: Field(...), key: String("value") }`).
/// In the runtime dispatch code, `.get("value")` extracts the inner value.
/// This is only used by the generated dispatch code in this crate (which is
/// dead code — only the `schema()` function is called).
pub trait OptionValueExt<T: Clone> {
    fn get(&self, _key: &str) -> T;
}
impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
    fn get(&self, _key: &str) -> T {
        self.clone().unwrap_or_default()
    }
}

impl<T: Clone + Default> OptionValueExt<T> for Option<&T> {
    fn get(&self, _key: &str) -> T {
        self.cloned().unwrap_or_default()
    }
}

pub mod auth_machine;
pub mod meerkat_machine;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;

use crate::identity::InputVariantId;
use crate::{MachineSchema, NamedTypeBinding, RustBinding};

pub struct MachineSchemaMetadata {
    pub named_types: Vec<NamedTypeBinding>,
    pub runtime_internal_inputs: Vec<InputVariantId>,
    pub ci_step_limit: Option<u32>,
}

impl MachineSchemaMetadata {
    pub fn attach_to(self, mut schema: MachineSchema) -> MachineSchema {
        schema.named_types = self.named_types;
        schema.runtime_internal_inputs = self.runtime_internal_inputs;
        schema.ci_step_limit = self.ci_step_limit;
        schema
    }

    pub fn with_ci_step_limit(mut self, ci_step_limit: u32) -> Self {
        self.ci_step_limit = Some(ci_step_limit);
        self
    }
}

pub const AUTH_MACHINE_PRODUCTION_RUST_CRATE: &str = "meerkat-runtime";
pub const AUTH_MACHINE_PRODUCTION_RUST_MODULE: &str = "auth_machine::dsl";
pub const MEERKAT_MACHINE_PRODUCTION_RUST_CRATE: &str = "meerkat-runtime";
pub const MEERKAT_MACHINE_PRODUCTION_RUST_MODULE: &str = "meerkat_machine::dsl";
pub const MOB_MACHINE_PRODUCTION_RUST_CRATE: &str = "meerkat-mob";
pub const MOB_MACHINE_PRODUCTION_RUST_MODULE: &str = "machines::mob_machine";

fn with_production_rust_binding(
    mut schema: MachineSchema,
    crate_name: &str,
    module: &str,
) -> MachineSchema {
    schema.rust = RustBinding {
        crate_name: crate_name.to_owned(),
        module: module.to_owned(),
    };
    schema
}

trait RuntimeInternalInputVariant: Copy {
    fn input_variant_id(self) -> InputVariantId;
}

macro_rules! runtime_internal_inputs {
    ($type_name:ident, $const_name:ident, [$($variant:ident),+ $(,)?]) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum $type_name {
            $($variant),+
        }

        const $const_name: &[$type_name] = &[
            $($type_name::$variant),+
        ];

        impl RuntimeInternalInputVariant for $type_name {
            fn input_variant_id(self) -> InputVariantId {
                InputVariantId::from_trusted_catalog_literal(match self {
                    $(Self::$variant => stringify!($variant),)+
                })
            }
        }
    };
}

fn input_variant_ids<T: RuntimeInternalInputVariant>(
    variants: &'static [T],
) -> Vec<InputVariantId> {
    variants
        .iter()
        .copied()
        .map(RuntimeInternalInputVariant::input_variant_id)
        .collect()
}

fn machine_schema_metadata(
    named_types: Vec<NamedTypeBinding>,
    runtime_internal_inputs: Vec<InputVariantId>,
) -> MachineSchemaMetadata {
    MachineSchemaMetadata {
        named_types,
        runtime_internal_inputs,
        ci_step_limit: None,
    }
}

pub fn dsl_auth_machine() -> MachineSchema {
    auth_machine_schema_metadata().attach_to(auth_machine::AuthMachineState::schema())
}

pub fn dsl_auth_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_auth_machine(),
        AUTH_MACHINE_PRODUCTION_RUST_CRATE,
        AUTH_MACHINE_PRODUCTION_RUST_MODULE,
    )
}

pub fn auth_machine_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(vec![NamedTypeBinding::string("AuthLifecyclePhase")], vec![])
}

pub fn dsl_meerkat_machine() -> MachineSchema {
    meerkat_machine_schema_metadata().attach_to(meerkat_machine::MeerkatMachineState::schema())
}

pub fn dsl_meerkat_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_meerkat_machine(),
        MEERKAT_MACHINE_PRODUCTION_RUST_CRATE,
        MEERKAT_MACHINE_PRODUCTION_RUST_MODULE,
    )
}

pub fn meerkat_machine_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::u64("BoundarySequence"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("CommsRuntimeId"),
            NamedTypeBinding::string("ConnectionRef"),
            NamedTypeBinding::string("DrainExitReason"),
            NamedTypeBinding::string("DrainMode"),
            NamedTypeBinding::string("DrainPhase"),
            NamedTypeBinding::string("ExternalToolSurfaceBaseState"),
            NamedTypeBinding::string("ExternalToolSurfaceDeltaOperation"),
            NamedTypeBinding::string("ExternalToolSurfaceDeltaPhase"),
            NamedTypeBinding::string("InboundPeerRequestState"),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string("InputAbandonReason"),
            NamedTypeBinding::string("InputLane"),
            NamedTypeBinding::string("InputPhase"),
            NamedTypeBinding::string("InputTerminalKind"),
            NamedTypeBinding::string("InteractionStreamState"),
            NamedTypeBinding::string("LiveTopologyPhase"),
            NamedTypeBinding::string("LlmRetryFailureKind"),
            NamedTypeBinding::string("McpServerId"),
            NamedTypeBinding::string("McpServerState"),
            NamedTypeBinding::string("MeerkatPhase"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("OperationId"),
            NamedTypeBinding::string("OperationKind"),
            NamedTypeBinding::string("OperationStatus"),
            NamedTypeBinding::string("OperationTerminalOutcomeKind"),
            NamedTypeBinding::string("OutboundPeerRequestState"),
            NamedTypeBinding::string("PeerCorrelationId"),
            NamedTypeBinding::string("PeerIngressOwnerKind"),
            NamedTypeBinding::string("PeerTerminalDisposition"),
            NamedTypeBinding::string("PostAdmissionSignalKind"),
            NamedTypeBinding::string("PreRunPhase"),
            NamedTypeBinding::string("Provider"),
            NamedTypeBinding::string("RealtimeBindingState"),
            NamedTypeBinding::string("RealtimeProductTurnPhase"),
            NamedTypeBinding::string("RealtimeProjectionFreshness"),
            NamedTypeBinding::string("RealtimeReconnectCycleState"),
            NamedTypeBinding::string("RealtimeReconnectPolicy"),
            NamedTypeBinding::string("RegistrationPhase"),
            NamedTypeBinding::string("RoutingApprovalParentKind"),
            NamedTypeBinding::string("RoutingApprovalPhase"),
            NamedTypeBinding::string("RoutingDenialReason"),
            NamedTypeBinding::string("RoutingImageOperationPhase"),
            NamedTypeBinding::string("RoutingImageTerminal"),
            NamedTypeBinding::string("RoutingSwitchTurnPhase"),
            NamedTypeBinding::string("RoutingSwitchTurnTerminal"),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("RuntimeNoticeKind"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("SessionLlmCapabilitySurface"),
            NamedTypeBinding::string("SessionLlmCapabilitySurfaceStatus"),
            NamedTypeBinding::string("SessionLlmIdentity"),
            NamedTypeBinding::string("SessionToolVisibilityDelta"),
            NamedTypeBinding::string("SessionToolVisibilityState"),
            NamedTypeBinding::string("SupervisorBindingKind"),
            NamedTypeBinding::string("SurfaceDeltaOperation"),
            NamedTypeBinding::string("SurfaceDeltaPhase"),
            NamedTypeBinding::string("SurfacePhase"),
            NamedTypeBinding::string("SurfaceId"),
            NamedTypeBinding::string("SurfacePendingOp"),
            NamedTypeBinding::string("SurfaceStagedOp"),
            NamedTypeBinding::string("ToolFilter"),
            NamedTypeBinding::string("ToolProvenance"),
            NamedTypeBinding::string("ToolSourceKind"),
            NamedTypeBinding::string("TurnCancellationReason"),
            NamedTypeBinding::type_path(
                "ToolVisibilityWitness",
                "crate::catalog::dsl::meerkat_machine::ToolVisibilityWitness",
            ),
            NamedTypeBinding::string("TurnPhase"),
            NamedTypeBinding::string("TurnPrimitiveKind"),
            NamedTypeBinding::string("TurnTerminalOutcome"),
            NamedTypeBinding::u64("TurnNumber"),
            NamedTypeBinding::string("WaitRequestId"),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::string("WorkOrigin"),
            // Wave-c C-6r: typed PeerEndpoint twin.
            NamedTypeBinding::type_path(
                "PeerEndpoint",
                "crate::catalog::dsl::meerkat_machine::PeerEndpoint",
            ),
            NamedTypeBinding::type_path(
                "PeerName",
                "crate::catalog::dsl::meerkat_machine::PeerName",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::meerkat_machine::PeerId"),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::meerkat_machine::PeerAddress",
            ),
            NamedTypeBinding::type_path(
                "PeerSigningKey",
                "crate::catalog::dsl::meerkat_machine::PeerSigningKey",
            ),
        ],
        input_variant_ids(MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS),
    )
}

runtime_internal_inputs!(
    MeerkatMachineRuntimeInternalInput,
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS,
    [
        AbandonInput,
        AbortOp,
        AbortLiveTopologyBeforeDetach,
        AcknowledgeTerminal,
        AddDirectPeerEndpoint,
        AdvanceSessionContext,
        ApplyLiveTopologyIdentity,
        ApplyLiveTopologyVisibility,
        ApplyMobPeerOverlay,
        AttachMobIngress,
        AttachSessionIngress,
        AuthorizeSupervisor,
        BeginLiveTopologyReconfigure,
        BeginRealtimeBinding,
        BeginRealtimeReconnectCycle,
        BindSupervisor,
        BoundaryComplete,
        BoundaryContinue,
        BudgetExhausted,
        CancelNow,
        CancelOp,
        CancelWaitAll,
        CancellationObserved,
        ChangeLane,
        ClassifyRealtimeClientInputSubmitted,
        ClassifyRealtimeMidTurnActivity,
        ClassifyRealtimeTurnTerminated,
        ClearLocalEndpoint,
        ClearRealtimeReconnectProgress,
        CoalesceInput,
        CommitDeferredNames,
        CommitVisibilityFilter,
        CompleteOp,
        CompleteUntilChangedSwitchTurnReconfigure,
        CompleteLiveTopology,
        ConsumeInput,
        ConsumeOnAccept,
        DetachIngress,
        DetachRealtimeBinding,
        DrainExitedClean,
        DrainExitedRespawnable,
        EnterExtraction,
        ExhaustRealtimeReconnectCycle,
        ExtractionStart,
        ExtractionValidationFailed,
        ExtractionValidationPassed,
        FailOp,
        FailLiveTopologyAfterDetach,
        FatalFailure,
        ForceCancelNoRun,
        IncrementAttemptCount,
        InteractionStreamAttached,
        InteractionStreamClosedEarly,
        InteractionStreamCompleted,
        InteractionStreamExpired,
        InteractionStreamReserved,
        LlmReturnedTerminal,
        LlmReturnedToolCalls,
        MarkApplied,
        MarkAppliedPendingConsumption,
        MarkLiveTopologyDetached,
        McpServerConnectPending,
        McpServerConnected,
        McpServerDisconnected,
        McpServerFailed,
        McpServerReload,
        ModelRoutingStatus,
        OpsBarrierSatisfied,
        PeerReadyOp,
        PeerRequestReceived,
        PeerRequestSent,
        PeerRequestTimedOut,
        PeerResponseProgressArrived,
        PeerResponseReplied,
        PeerResponseTerminalArrived,
        PrimitiveApplied,
        ProductOutputStarted,
        ProductTurnCommitted,
        ProductTurnInFlight,
        ProductTurnInterrupted,
        ProductTurnTerminal,
        ProgressReportedOp,
        ProjectRealtimeIntent,
        QueueAccepted,
        RecordBoundarySeq,
        PublishLocalEndpoint,
        PublishRealtimeSignal,
        RealtimeProjectionAdvanceObserved,
        RealtimeProjectionRefreshed,
        RealtimeProjectionReset,
        RecoverableFailure,
        RecoverInputLifecycle,
        RegisterOp,
        RegisterPendingOps,
        RemoveDirectPeerEndpoint,
        ReplaceRealtimeBinding,
        RequestCancelAfterBoundary,
        RequestFiniteSwitchTurn,
        RequestUntilChangedSwitchTurn,
        RequestWaitAll,
        RequireRealtimeReattach,
        RequireRealtimeReattachForAuthority,
        RetireCompletedOp,
        RetireRequestedOp,
        RetryRequested,
        RevokeSupervisor,
        RollbackStaged,
        RunCancelled,
        RunCompleted,
        RunFailed,
        RuntimeExecutorExited,
        SatisfyWaitAll,
        ScheduleRealtimeReconnectRetry,
        SetModelRoutingBaseline,
        SpawnDrain,
        StageDeferredNames,
        StageForRun,
        StageVisibilityFilter,
        StartConversationRun,
        StartImmediateAppend,
        StartImmediateContext,
        StartOp,
        SteerAccepted,
        StopDrain,
        SupersedeInput,
        SurfaceApplyBoundary,
        SurfaceCallFinished,
        SurfaceCallStarted,
        SurfaceFinalizeRemovalClean,
        SurfaceFinalizeRemovalForced,
        SurfaceMarkPendingFailed,
        SurfaceMarkPendingSucceeded,
        SurfaceSnapshotAligned,
        SurfaceShutdown,
        SurfaceRegister,
        SurfaceStageAdd,
        SurfaceStageReload,
        SurfaceStageRemove,
        SyncVisibilityRevisions,
        SupervisorTrustEdgePublishFailed,
        SupervisorTrustEdgePublished,
        SupervisorTrustEdgeRevokeFailed,
        SupervisorTrustEdgeRevoked,
        TerminateOp,
        TimeBudgetExceeded,
        ToolCallsResolved,
        TurnLimitReached,
    ]
);

pub fn dsl_mob_machine() -> MachineSchema {
    mob_machine_schema_metadata().attach_to(mob_machine::MobMachineState::schema())
}

pub fn dsl_mob_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_mob_machine(),
        MOB_MACHINE_PRODUCTION_RUST_CRATE,
        MOB_MACHINE_PRODUCTION_RUST_MODULE,
    )
}

pub fn mob_machine_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentIdentity"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("BranchId"),
            NamedTypeBinding::type_path(
                "ExternalPeerEdge",
                "crate::catalog::dsl::mob_machine::ExternalPeerEdge",
            ),
            NamedTypeBinding::type_path(
                "ExternalPeerEndpoint",
                "crate::catalog::dsl::mob_machine::ExternalPeerEndpoint",
            ),
            NamedTypeBinding::string("FlowNodeId"),
            NamedTypeBinding::string("FrameNodeKey"),
            NamedTypeBinding::string("FrameId"),
            NamedTypeBinding::string("KickoffPhase"),
            NamedTypeBinding::string("LoopId"),
            NamedTypeBinding::string("LoopInstanceId"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("MobMemberState"),
            NamedTypeBinding::string("MobPhase"),
            NamedTypeBinding::string("MobTask"),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("RunStepKey"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("StepId"),
            NamedTypeBinding::string("TaskId"),
            NamedTypeBinding::string("TaskStatus"),
            NamedTypeBinding::string("WiringEdge"),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::mob_machine::PeerAddress",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::mob_machine::PeerId"),
            NamedTypeBinding::type_path("PeerName", "crate::catalog::dsl::mob_machine::PeerName"),
            NamedTypeBinding::type_path(
                "PeerSigningKey",
                "crate::catalog::dsl::mob_machine::PeerSigningKey",
            ),
        ],
        input_variant_ids(MOB_MACHINE_RUNTIME_INTERNAL_INPUTS),
    )
    .with_ci_step_limit(1)
}

runtime_internal_inputs!(
    MobMachineRuntimeInternalInput,
    MOB_MACHINE_RUNTIME_INTERNAL_INPUTS,
    [
        AuthorizeFlowFrameReducerCommand,
        AuthorizeFlowRunReducerCommand,
        AuthorizeLoopIterationReducerCommand,
        CreateFrameSeed,
        CreateLoopSeed,
        CreateRunSeed,
        KickoffCancelRequested,
        KickoffClear,
        KickoffMarkPending,
        KickoffMarkStarting,
        KickoffResolveCallbackPending,
        KickoffResolveFailed,
        KickoffResolveStarted,
        RecordLoopBodyFrameCompleted,
        RecordLoopUntilConditionFailed,
        RecordLoopUntilConditionMet,
        StartupMarkReady,
        SessionIngressDetachFailedForMobDestroy,
        SessionIngressDetachedForMobDestroy,
    ]
);

pub fn dsl_schedule_lifecycle_machine() -> MachineSchema {
    schedule_lifecycle_schema_metadata()
        .attach_to(schedule_lifecycle::ScheduleLifecycleMachineState::schema())
}

pub fn schedule_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        // Schedule-side reciprocal ack tracks `Set<OccurrenceId>` and
        // receives `ConfirmOccurrencesSuperseded { occurrence_id }` from
        // the occurrence authority; both sides must agree on the atom.
        vec![
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string("ScheduleLifecycleState"),
        ],
        vec![],
    )
}

pub fn dsl_occurrence_lifecycle_machine() -> MachineSchema {
    occurrence_lifecycle_schema_metadata()
        .attach_to(occurrence_lifecycle::OccurrenceLifecycleMachineState::schema())
}

pub fn occurrence_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string("ClaimToken"),
            NamedTypeBinding::string("DeliveryReceipt"),
            NamedTypeBinding::string("OccurrenceFailureClass"),
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string("OccurrenceLifecycleState"),
            NamedTypeBinding::string("ScheduleId"),
        ],
        vec![],
    )
}
