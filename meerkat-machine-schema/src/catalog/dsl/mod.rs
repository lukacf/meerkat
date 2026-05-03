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
use crate::{
    MachineSchema, NamedTypeBinding, RustBinding, TypePathEnumStructuralVariant,
    TypePathStructField,
};

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
pub const SCHEDULE_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-schedule";
pub const SCHEDULE_LIFECYCLE_PRODUCTION_RUST_MODULE: &str = "machines::schedule_lifecycle";
pub const OCCURRENCE_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-schedule";
pub const OCCURRENCE_LIFECYCLE_PRODUCTION_RUST_MODULE: &str = "machines::occurrence_lifecycle";

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
    (
        $type_name:ident,
        $const_name:ident,
        $variant_module:ident::$variant_type:ident,
        [$($variant:ident),+ $(,)?]
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum $type_name {
            $($variant),+
        }

        const $const_name: &[$type_name] = &[
            $($type_name::$variant),+
        ];

        impl $type_name {
            fn input_variant(self) -> $variant_module::$variant_type {
                match self {
                    $(Self::$variant => $variant_module::$variant_type::$variant,)+
                }
            }
        }

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
    machine_schema_metadata(
        vec![NamedTypeBinding::string_enum(
            "AuthLifecyclePhase",
            &[
                "Valid",
                "Expiring",
                "Refreshing",
                "ReauthRequired",
                "Released",
            ],
        )],
        vec![],
    )
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
            NamedTypeBinding::string_enum(
                meerkat_core::turn_execution_authority::ContentShape::SCHEMA_TYPE_NAME,
                &meerkat_core::turn_execution_authority::ContentShape::SCHEMA_VARIANTS,
            ),
            NamedTypeBinding::string_enum(
                "DrainExitReason",
                &[
                    "IdleTimeout",
                    "Dismissed",
                    "Failed",
                    "Aborted",
                    "SessionShutdown",
                ],
            ),
            NamedTypeBinding::string_enum(
                "DrainMode",
                &["Timed", "AttachedSession", "PersistentHost"],
            ),
            NamedTypeBinding::string_enum(
                "DrainPhase",
                &["Inactive", "Running", "Stopped", "ExitedRespawnable"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceBaseState",
                &["Absent", "Active", "Removing", "Removed"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceDeltaOperation",
                &["None", "Add", "Remove", "Reload"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceDeltaPhase",
                &["None", "Pending", "Applied", "Draining", "Failed", "Forced"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceFailureCause",
                &["PendingFailed", "SurfaceDraining", "SurfaceUnavailable"],
            ),
            NamedTypeBinding::string_enum("InboundPeerRequestState", &["Received", "Replied"]),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string_enum(
                "InputAbandonReason",
                &[
                    "Retired",
                    "Reset",
                    "Stopped",
                    "Destroyed",
                    "Cancelled",
                    "MaxAttemptsExhausted",
                ],
            ),
            NamedTypeBinding::string_enum("InputLane", &["Queue", "Steer"]),
            NamedTypeBinding::string_enum(
                "InputPhase",
                &[
                    "Queued",
                    "Staged",
                    "Applied",
                    "AppliedPendingConsumption",
                    "Consumed",
                    "Superseded",
                    "Coalesced",
                    "Abandoned",
                ],
            ),
            NamedTypeBinding::string_enum(
                "InputTerminalKind",
                &["Consumed", "Superseded", "Coalesced", "Abandoned"],
            ),
            NamedTypeBinding::string_enum(
                "InteractionStreamState",
                &[
                    "Reserved",
                    "Attached",
                    "Completed",
                    "Expired",
                    "ClosedEarly",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveTopologyPhase",
                &[
                    "Idle",
                    "Reconfiguring",
                    "Detached",
                    "HostIdentityApplied",
                    "HostVisibilityApplied",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LlmRetryFailureKind",
                &[
                    "RateLimited",
                    "NetworkTimeout",
                    "CallTimeout",
                    "RetryableProviderError",
                ],
            ),
            NamedTypeBinding::string("McpServerId"),
            NamedTypeBinding::string_enum(
                "McpServerState",
                &["PendingConnect", "Connected", "Failed", "Disconnected"],
            ),
            NamedTypeBinding::string("MeerkatPhase"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("OperationId"),
            NamedTypeBinding::string_enum("OperationKind", &["MobMemberChild", "BackgroundToolOp"]),
            NamedTypeBinding::string_enum(
                "OperationStatus",
                &[
                    "Absent",
                    "Provisioning",
                    "Running",
                    "Retiring",
                    "Completed",
                    "Failed",
                    "Aborted",
                    "Cancelled",
                    "Retired",
                    "Terminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OperationTerminalOutcomeKind",
                &[
                    "Completed",
                    "Failed",
                    "Aborted",
                    "Cancelled",
                    "Retired",
                    "Terminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OutboundPeerRequestState",
                &[
                    "Sent",
                    "AcceptedProgress",
                    "Completed",
                    "Failed",
                    "TimedOut",
                ],
            ),
            NamedTypeBinding::string("PeerCorrelationId"),
            NamedTypeBinding::string_enum(
                "PeerIngressAdmittedKind",
                &["Message", "Request", "Response", "Ack", "PlainEvent"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressAuthClass",
                &["Required", "SupervisorBridgeExempt"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressEnvelopeClass",
                &["Message", "Request", "Lifecycle", "Response", "Ack"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressInputClass",
                &[
                    "ActionableMessage",
                    "ActionableRequest",
                    "ResponseProgress",
                    "ResponseTerminal",
                    "PeerLifecycleAdded",
                    "PeerLifecycleRetired",
                    "PeerLifecycleUnwired",
                    "SilentRequest",
                    "Ack",
                    "PlainEvent",
                ],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressLifecycleClass",
                &["PeerAdded", "PeerRetired", "PeerUnwired"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressOwnerKind",
                &["Unattached", "SessionOwned", "MobOwned"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressResponseStatus",
                &["Accepted", "Completed", "Failed"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressResponseTerminality",
                &["Progress", "TerminalCompleted", "TerminalFailed"],
            ),
            NamedTypeBinding::string_enum("PeerTerminalDisposition", &["Completed", "Failed"]),
            NamedTypeBinding::string_enum(
                "PostAdmissionSignalKind",
                &[
                    "WakeLoop",
                    "InterruptYielding",
                    "RequestImmediateProcessing",
                ],
            ),
            NamedTypeBinding::string_enum("PreRunPhase", &["Idle", "Attached", "Retired"]),
            NamedTypeBinding::string_enum(
                "Provider",
                &["Anthropic", "OpenAI", "Gemini", "SelfHosted", "Other"],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeBindingState",
                &[
                    "Unbound",
                    "BindingNotReady",
                    "BindingReady",
                    "ReplacementPending",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeProductTurnPhase",
                &[
                    "Idle",
                    "AwaitingProgress",
                    "Committed",
                    "OutputStarted",
                    "Preemptible",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeProjectionFreshness",
                &["Clean", "StaleDeferred", "StaleImmediate"],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeReconnectCycleState",
                &["Idle", "Reconnecting", "Exhausted"],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeReconnectPolicy",
                &["CleanExit", "ReattachAndRecover"],
            ),
            NamedTypeBinding::string_enum("RegistrationPhase", &["Queuing", "Active"]),
            NamedTypeBinding::string_enum(
                "RoutingApprovalParentKind",
                &["SwitchTurn", "ImageOperation"],
            ),
            NamedTypeBinding::string_enum(
                "RoutingApprovalPhase",
                &[
                    "Pending",
                    "PresentedToUser",
                    "Approved",
                    "Denied",
                    "SurfaceDetached",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingDenialReason",
                &[
                    "CapabilityPolicy",
                    "ApprovalRequiredButUnavailable",
                    "DeniedDuringApproval",
                    "ScopedOverrideConflict",
                    "RealtimeTransportConflict",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageOperationPhase",
                &[
                    "Requested",
                    "PlanResolved",
                    "ScopedOverrideActive",
                    "ProviderCallInFlight",
                    "ResultCommitted",
                    "RestoringScopedOverride",
                    "Terminal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageTerminal",
                &[
                    "Generated",
                    "Denied",
                    "EmptyResult",
                    "RefusedByProvider",
                    "SafetyFiltered",
                    "Failed",
                    "Cancelled",
                    "Timeout",
                    "ScopedRestoreFailed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingSwitchTurnPhase",
                &[
                    "Requested",
                    "PendingForBoundary",
                    "ActiveFiniteOverride",
                    "ApplyingPersistentReconfigure",
                    "Terminal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingSwitchTurnTerminal",
                &[
                    "Denied",
                    "ConsumedAndRestored",
                    "PersistentReconfigureApplied",
                ],
            ),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string_enum(
                "RuntimeApplyFailureCause",
                &[
                    "Unknown",
                    "PrimitiveRejected",
                    "RuntimeContextApply",
                    "RuntimeTurn",
                    "HookDenied",
                    "HookRuntimeFailure",
                    "ExecutorStopped",
                    "ExecutorControlFailed",
                    "ExecutorInternal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeNoticeKind",
                &["Drain", "Reset", "Stop", "Exit", "Recover"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeEffectKind",
                &["CancelAfterBoundary", "StopRuntimeExecutor"],
            ),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("SessionLlmCapabilitySurface"),
            NamedTypeBinding::string_enum(
                "SessionLlmCapabilitySurfaceStatus",
                &["Unresolved", "Resolved"],
            ),
            NamedTypeBinding::string("SessionLlmIdentity"),
            NamedTypeBinding::string("SessionToolVisibilityDelta"),
            NamedTypeBinding::string("SessionToolVisibilityState"),
            NamedTypeBinding::string_enum("SupervisorBindingKind", &["Unbound", "Bound"]),
            NamedTypeBinding::string("SurfaceDeltaOperation"),
            NamedTypeBinding::string("SurfaceDeltaPhase"),
            NamedTypeBinding::string_enum("SurfacePhase", &["Operating", "Shutdown"]),
            NamedTypeBinding::string("SurfaceId"),
            NamedTypeBinding::string_enum("SurfacePendingOp", &["None", "Add", "Reload"]),
            NamedTypeBinding::string_enum("SurfaceStagedOp", &["None", "Add", "Remove", "Reload"]),
            NamedTypeBinding::type_path_enum_with_structural_variants(
                "ToolFilter",
                "crate::catalog::dsl::meerkat_machine::ToolFilter",
                &["All"],
                vec![
                    TypePathEnumStructuralVariant::string_set("Allow", "names"),
                    TypePathEnumStructuralVariant::string_set("Deny", "names"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "ToolProvenance",
                "crate::catalog::dsl::meerkat_machine::ToolProvenance",
                vec![
                    TypePathStructField::named("kind", "ToolSourceKind"),
                    TypePathStructField::string("source_id"),
                ],
            ),
            NamedTypeBinding::string_enum(
                "ToolSourceKind",
                &[
                    "Builtin",
                    "Shell",
                    "Comms",
                    "Memory",
                    "Schedule",
                    "Mob",
                    "MobTasks",
                    "Callback",
                    "Mcp",
                    "RustBundle",
                ],
            ),
            NamedTypeBinding::string_enum("TurnCancellationReason", &["Observed"]),
            NamedTypeBinding::type_path_field_presence_set(
                "ToolVisibilityWitness",
                "crate::catalog::dsl::meerkat_machine::ToolVisibilityWitness",
                &["stable_owner_key", "last_seen_provenance"],
            ),
            NamedTypeBinding::string_enum(
                "TurnPhase",
                &[
                    "Ready",
                    "ApplyingPrimitive",
                    "CallingLlm",
                    "WaitingForOps",
                    "DrainingBoundary",
                    "Extracting",
                    "ErrorRecovery",
                    "Cancelling",
                    "Completed",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TurnPrimitiveKind",
                &[
                    "None",
                    "ConversationTurn",
                    "ImmediateAppend",
                    "ImmediateContextAppend",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TurnTerminalOutcome",
                &[
                    "None",
                    "Completed",
                    "Failed",
                    "Cancelled",
                    "BudgetExhausted",
                    "TimeBudgetExceeded",
                    "StructuredOutputValidationFailed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TurnTerminalCauseKind",
                &[
                    "Unknown",
                    "HookDenied",
                    "HookFailure",
                    "LlmFailure",
                    "ToolFailure",
                    "StructuredOutputValidationFailed",
                    "BudgetExhausted",
                    "TimeBudgetExceeded",
                    "TurnLimitReached",
                    "RuntimeApplyFailure",
                    "FatalFailure",
                ],
            ),
            NamedTypeBinding::u64("TurnNumber"),
            NamedTypeBinding::string("WaitRequestId"),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::string_enum("WorkOrigin", &["External", "Internal", "Ingest"]),
            // Wave-c C-6r: typed PeerEndpoint twin.
            NamedTypeBinding::type_path_struct(
                "PeerEndpoint",
                "crate::catalog::dsl::meerkat_machine::PeerEndpoint",
                vec![
                    TypePathStructField::named("name", "PeerName"),
                    TypePathStructField::named("peer_id", "PeerId"),
                    TypePathStructField::named("address", "PeerAddress"),
                    TypePathStructField::named("signing_key", "PeerSigningKey"),
                ],
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
    .with_ci_step_limit(1)
}

runtime_internal_inputs!(
    MeerkatMachineRuntimeInternalInput,
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS,
    meerkat_machine::MeerkatMachineInputVariant,
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
        InterruptCurrentRun,
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
        RollbackRun,
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

pub fn meerkat_machine_runtime_internal_input_variants()
-> Vec<meerkat_machine::MeerkatMachineInputVariant> {
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS
        .iter()
        .copied()
        .map(MeerkatMachineRuntimeInternalInput::input_variant)
        .collect()
}

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
            NamedTypeBinding::string_enum("CollectionPolicyKind", &["All", "Any", "Quorum"]),
            NamedTypeBinding::string_enum("DependencyMode", &["All", "Any"]),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::string_enum(
                "FlowFrameReducerCommandKind",
                &[
                    "StartRootFrame",
                    "StartBodyFrame",
                    "AdmitNextReadyNode",
                    "CompleteNode",
                    "RecordNodeOutput",
                    "FailNode",
                    "SkipNode",
                    "CancelNode",
                    "SealFrame",
                ],
            ),
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
            NamedTypeBinding::string_enum("FlowNodeKind", &["Step", "Loop"]),
            NamedTypeBinding::string_enum(
                "FlowRunReducerCommandKind",
                &[
                    "CreateRun",
                    "StartRun",
                    "DispatchStep",
                    "CompleteStep",
                    "RecordStepOutput",
                    "ConditionPassed",
                    "ConditionRejected",
                    "FailStep",
                    "SkipStep",
                    "ProjectFrameStepStatus",
                    "CancelStep",
                    "RegisterTargets",
                    "RecordTargetSuccess",
                    "RecordTargetTerminalFailure",
                    "RecordTargetCanceled",
                    "RecordTargetFailure",
                    "RegisterReadyFrame",
                    "PumpNodeScheduler",
                    "RegisterPendingBodyFrame",
                    "PumpFrameScheduler",
                    "NodeExecutionReleased",
                    "FrameTerminated",
                    "TerminalizeCompleted",
                    "TerminalizeFailed",
                    "TerminalizeCanceled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "FlowRunStatus",
                &[
                    "Absent",
                    "Pending",
                    "Running",
                    "Completed",
                    "Failed",
                    "Canceled",
                ],
            ),
            NamedTypeBinding::string("FrameNodeKey"),
            NamedTypeBinding::string("FrameId"),
            NamedTypeBinding::string_enum("FrameScope", &["Root", "Body"]),
            NamedTypeBinding::string_enum(
                "FrameStatus",
                &["Running", "Completed", "Failed", "Canceled"],
            ),
            NamedTypeBinding::string_enum(
                "KickoffIntent",
                &[
                    "Pending",
                    "Starting",
                    "Started",
                    "CallbackPending",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "KickoffPhase",
                &[
                    "Pending",
                    "Starting",
                    "CallbackPending",
                    "Started",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LoopIterationReducerCommandKind",
                &[
                    "StartLoop",
                    "BodyFrameStarted",
                    "BodyFrameCompleted",
                    "BodyFrameFailed",
                    "BodyFrameCanceled",
                    "UntilConditionMet",
                    "UntilConditionFailed",
                    "CancelLoop",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LoopIterationStage",
                &[
                    "AwaitingBodyFrame",
                    "BodyFrameActive",
                    "AwaitingUntilEvaluation",
                ],
            ),
            NamedTypeBinding::string("LoopId"),
            NamedTypeBinding::string("LoopInstanceId"),
            NamedTypeBinding::string_enum(
                "LoopStatus",
                &["Running", "Completed", "Exhausted", "Failed", "Canceled"],
            ),
            NamedTypeBinding::string_enum(
                "MemberLifecycleKind",
                &[
                    "Spawned",
                    "Retiring",
                    "Retired",
                    "Reset",
                    "Respawned",
                    "Completed",
                    "Destroyed",
                ],
            ),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string_enum("MobMemberState", &["Active", "Retiring"]),
            NamedTypeBinding::string_enum(
                "MobPhase",
                &["Running", "Stopped", "Completed", "Destroyed"],
            ),
            NamedTypeBinding::string("MobTask"),
            NamedTypeBinding::string_enum(
                "NodeRunStatus",
                &[
                    "Pending",
                    "Ready",
                    "Running",
                    "Completed",
                    "Failed",
                    "Skipped",
                    "Canceled",
                ],
            ),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("RunStepKey"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("StepId"),
            NamedTypeBinding::string_enum(
                "StepRunStatus",
                &["Dispatched", "Completed", "Failed", "Skipped", "Canceled"],
            ),
            NamedTypeBinding::string("TaskId"),
            NamedTypeBinding::string_enum(
                "TaskStatus",
                &["Pending", "InProgress", "Completed", "Cancelled"],
            ),
            NamedTypeBinding::string("WiringEdge"),
            NamedTypeBinding::string_enum("WiringLifecycleKind", &["Wired", "Unwired"]),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::string_enum("WorkOrigin", &["External", "Internal", "Ingest"]),
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
    mob_machine::MobMachineInputVariant,
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

pub fn mob_machine_runtime_internal_input_variants() -> Vec<mob_machine::MobMachineInputVariant> {
    MOB_MACHINE_RUNTIME_INTERNAL_INPUTS
        .iter()
        .copied()
        .map(MobMachineRuntimeInternalInput::input_variant)
        .collect()
}

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
            NamedTypeBinding::string_enum("MisfirePolicy", &["Skip", "CatchUpWithin"]),
            NamedTypeBinding::string_enum("MissingTargetPolicy", &["MarkMisfired", "Skip"]),
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string_enum("OverlapPolicy", &["AllowConcurrent", "SkipIfRunning"]),
            NamedTypeBinding::string_enum(
                "ScheduleLifecycleState",
                &["Active", "Paused", "Deleted"],
            ),
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
            NamedTypeBinding::string_enum(
                "OccurrenceFailureClass",
                &[
                    "TargetMaterializationFailed",
                    "TargetMissing",
                    "TargetBusy",
                    "RuntimeRejected",
                    "MobRejected",
                    "LeaseLost",
                    "TransportError",
                    "InternalError",
                ],
            ),
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string_enum(
                "OccurrenceLifecycleState",
                &[
                    "Pending",
                    "Claimed",
                    "Dispatching",
                    "AwaitingCompletion",
                    "Completed",
                    "Skipped",
                    "Misfired",
                    "Superseded",
                    "DeliveryFailed",
                ],
            ),
            NamedTypeBinding::string("ScheduleId"),
        ],
        vec![],
    )
}
