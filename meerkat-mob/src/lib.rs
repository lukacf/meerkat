//! Meerkat Mob - Multi-agent orchestration runtime.
//!
//! This crate provides the runtime for orchestrating multiple agents as a
//! collaborative mob. It handles member spawning, wiring, lifecycle
//! management, and shared task coordination.
//!
//! # Architecture
//!
//! `meerkat-mob` is a plugin crate with a one-way dependency on the Meerkat
//! platform. No core Meerkat crate depends on this crate.
//!
//! Key types:
//! - [`MobDefinition`] - Describes mob structure (profiles, wiring, skills)
//! - [`MobEvent`] / [`MobEventKind`] - Structural state changes
//! - [`MobEventStore`] - Persistence trait for mob events
//! - [`MobStorage`] - Storage bundle for a mob

#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    clippy::collapsible_if,
    clippy::expect_used,
    clippy::if_not_else,
    clippy::implicit_clone,
    clippy::large_futures,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_clone,
    clippy::redundant_feature_names,
    clippy::unnecessary_to_owned
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::io_other_error,
        clippy::await_holding_lock
    )
)]

/// ATIF trajectory export vocabulary, for hosts that assemble mob member
/// trajectories into one document. Gated because nothing in this crate consumes
/// it (feature `atif`).
#[cfg(feature = "atif")]
pub use meerkat_atif as atif;

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;

    pub mod time {
        use std::future::Future;

        pub use meerkat_core::time_compat::Instant;
        pub use tokio_with_wasm::alias::time::*;

        pub async fn timeout_at<F>(
            deadline: Instant,
            future: F,
        ) -> Result<F::Output, tokio_with_wasm::alias::time::Elapsed>
        where
            F: Future,
        {
            tokio_with_wasm::alias::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                future,
            )
            .await
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub mod tokio {
    pub use ::tokio::*;
}

#[doc(hidden)]
pub mod adaptive;
pub mod backend;
mod build;
pub mod control_policy;
pub mod coordination;
pub mod definition;
pub mod error;
pub mod event;
pub mod forked_participant;
#[doc(hidden)]
pub mod generated;
pub mod identity;
pub mod ids;
pub mod launch;
#[doc(hidden)]
pub mod machines;
mod mob_machine;
mod portable_profile;
pub mod profile;
mod roster;
pub mod run;
pub mod runtime;
pub mod runtime_mode;
mod snapshot;
pub mod spec;
pub mod storage;
pub mod store;
pub mod temporary_council;
pub mod validate;
pub mod workgraph_attention;
pub mod workgraph_flow;

// Re-exports for convenience
pub use backend::{MobBackendKind, RuntimeBinding};
pub use control_policy::{
    CommandAuthority, CommandAuthorityKind, ControlScope, MobControlPrincipal, OperatorGrant,
    ResolvedControlPolicy, ScopeDenial,
};
pub use coordination::{
    CoordinationOwner, CoordinationRecordRefs, CoordinationResourceRef, MobCoordinationError,
    MobCoordinationEvent, MobCoordinationEventKind, MobCoordinationSnapshot, ResourceClaim,
    ResourceClaimId, ResourceClaimKind, ResourceClaimStatus, WorkIntent, WorkIntentId,
    WorkIntentStatus,
};
pub use definition::{MobDefinition, MobDefinitionSourceIdentity, MobDefinitionSourceKind};
pub use error::{
    FlowStepDispatchRejectKind, ForkSourceUnavailableCause, ForkedParticipantLeaseOperation,
    ForkedParticipantOwnerHostRejection, ForkedParticipantSourceRejection,
    MemberProvisionFailureCause, MobError, MobFailureClass, RuntimeEffectKind,
};
pub use event::{
    AttributedEvent, FlowCancelClass, MemberWireEdge, MobEvent, MobEventKind, NewMobEvent,
};
pub use forked_participant::ForkedParticipantSourceRuntime;
pub use identity::{
    AdoptMemberIdentityDeclaration, AdoptMemberIdentityDeclarationResult,
    ApplyMemberToolDeclaration, ApplyMemberToolDeclarationResult, CallbackToolSetDeclaration,
    DesiredExecution, DesiredExternalAddress, DesiredIdentityEdge, DesiredInitialDelivery,
    DesiredLocalCallbackTool, DesiredMemberMaterial, DesiredMemberOverlay, DesiredMemberSpec,
    DesiredSessionAuthorityPolicy, DesiredSessionTarget, IdentityActuationPermit,
    IdentityActuatorTarget, IdentityAdoptionId, IdentityAdoptionOutcome,
    IdentityAdoptionPrecondition, IdentityAdoptionReceipt, IdentityAuthorityCondition,
    IdentityConvergenceCondition, IdentityConvergenceDirective, IdentityConvergenceMode,
    IdentityConvergenceResolutionId, IdentityConvergenceResolutionOutcome,
    IdentityConvergenceResolutionReceipt, IdentityConvergenceStatus, IdentityDeclarationScopeId,
    IdentityExternalCeremonyCondition, IdentityExternalTrustCondition,
    IdentityInitialDeliveryCondition, IdentityIntent, IdentityIntentError,
    IdentityIntentMutationReceipt, IdentityIntentRecord, IdentityLeaseClaim,
    IdentityLeaseClaimOutcome, IdentityLeaseCondition, IdentityLeaseRecord, IdentityOperationKind,
    IdentityOperationReceipt, IdentityOperationReceiptInsertOutcome,
    IdentityOperationReceiptPayload, IdentityOperationSlot, IdentityOperationSubject,
    IdentityProfileMemberDeclaration, IdentityReceiptCondition, IdentityReconcileDecision,
    IdentityReconcileFacts, IdentityResourceCondition, IdentityResourceObservation,
    IdentityRetirementPlan, IdentitySessionCondition, IdentitySessionObservation,
    IdentitySessionStoreAuthority, IdentityStoredObservation, IdentityTargetObservationVersion,
    MemberToolAccessConstraint, MemberToolAccessDeclaration, MemberToolCommitOutcome,
    MemberToolDeclaration, MemberToolMutationId, ResolveIdentityConvergenceBlock,
    ResolveIdentityConvergenceBlockResult, classify_identity_reconciliation,
};
pub use ids::{
    AgentIdentity, AgentRuntimeId, BranchId, FenceToken, FlowId, FlowNodeId, FrameId, Generation,
    LoopId, LoopInstanceId, MobId, PlacedSpawnId, ProfileName, RespawnTopologyPeerId, RunId,
    StepId, WorkOrigin, WorkRef, WorkSpec,
};
pub use launch::{ForkContext, MemberLaunchMode};
#[doc(hidden)]
pub use mob_machine::{
    MobMachineCatalogInput, MobMachineCommandClassification, MobMachineCommandClassificationRecord,
    MobMachineCommandVariant, MobMachineRuntimeInternalClassificationRecord,
    MobMachineRuntimeInternalReason, MobMachineShellMechanicReason,
    canonical_mob_machine_command_classifications,
    canonical_mob_machine_command_input_variant_manifest, canonical_mob_machine_command_manifest,
    canonical_mob_machine_runtime_internal_classifications,
    canonical_mob_machine_runtime_internal_input_variant_manifest,
    canonical_mob_machine_runtime_internal_manifest,
};
pub use workgraph_attention::{
    lower_agent_identity_attention_target, lower_agent_identity_owner_key,
};
pub use workgraph_flow::{
    AbandonUncertainWorkGraphFlowRequest, LaunchWorkGraphFlowRequest, WorkGraphFlowAbandonResult,
    WorkGraphFlowAdmission, WorkGraphFlowBridge, WorkGraphFlowBridgeError,
    WorkGraphFlowCustodyGuard, WorkGraphFlowExecutionAuthority, WorkGraphFlowHost,
    WorkGraphFlowLaunchResult, WorkGraphFlowObservationAuthority, WorkGraphFlowReconcileResult,
};

#[doc(hidden)]
pub mod machine_schema_exports {
    pub fn mob_machine_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::mob_machine_schema_metadata()
            .attach_to(crate::machines::mob_machine::MobMachineState::schema())
    }

    /// Production-schema parity export for the non-canonical scoped authority
    /// (plan §21.5). `attach_to` keeps the expansion's own rust binding, which
    /// is what `meerkat-mob/tests/mob_host_binding_authority.rs` compares
    /// against the catalog-side production-schema variant.
    pub fn mob_host_binding_authority_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::mob_host_binding_authority_schema_metadata()
            .attach_to(
                crate::machines::mob_host_binding_authority::MobHostBindingAuthorityState::schema(),
            )
    }

    pub fn temporary_council_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::temporary_council_lifecycle_schema_metadata()
            .attach_to(
                crate::machines::temporary_council_lifecycle::TemporaryCouncilLifecycleMachineState::schema(),
            )
    }

    pub fn forked_participant_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::forked_participant_lifecycle_schema_metadata()
            .attach_to(
                crate::machines::forked_participant_lifecycle::ForkedParticipantLifecycleMachineState::schema(),
            )
    }
}

#[doc(hidden)]
pub use machines::mob_host_binding_authority::{
    canonical_mob_host_binding_authority_runtime_internal_classifications,
    canonical_mob_host_binding_authority_runtime_internal_input_variant_manifest,
};

pub use profile::{
    Profile, ProfileBinding, ProfileSource, ResumeOverrideField, SpawnTooling, ToolConfig,
};
pub use roster::{MobMemberKickoffPhase, MobMemberKickoffSnapshot};
pub use run::{
    CreateMobAdmission, FailureLedgerEntry, FlowContext, FlowRunConfig, FrameSnapshot,
    LoopContextHistory, LoopIterationLedgerEntry, LoopSnapshot, MobFlowRunPublicResultClass,
    MobRun, MobRunStatus, ProfileMutationAdmission, StepLedgerEntry, StepRunStatus,
    mob_machine_create_mob_admission, mob_machine_profile_mutation_admission,
    mob_machine_run_public_result_class, mob_machine_run_status_is_terminal,
    mob_machine_step_status_is_terminal,
};
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::FactoryChainSpawnBasePromptSource;
pub use runtime::RestoreIncompatible;
pub use runtime::bridge::{MobBoundMemberRuntimeBridge, MobMemberRuntimeBridge};
pub use runtime::bridge_protocol::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeDeliveryOutcome, BridgeDeliveryPayload, BridgeDeliveryRejectionCause,
    BridgeDeliveryResponse, BridgeDestroyResponse, BridgeHardCancelPayload,
    BridgeMemberRuntimeState, BridgeMobPeerOverlayHandoff, BridgeObservationResponse,
    BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload, BridgeReply,
    BridgeRetireResponse, BridgeSupervisorPayload,
};
#[cfg(feature = "runtime-adapter")]
pub use runtime::local_bridge::LocalMobRuntimeBridge;
#[cfg(feature = "runtime-adapter")]
pub use runtime::run_mobpack_callable;
pub use runtime::{
    AdaptiveDriverCapability, AdaptiveLayerAdmission, AdaptiveLayerAdmissionRequest,
    AdaptiveLayerAttempt, AdaptiveLayerDisposition, AdaptiveLayerPhaseView,
    AdaptiveLayerResultDigest, AdaptiveLayerRetention, AdaptiveLayerRunStart,
    AdaptiveLayerSetupFault, AdaptiveLayerSetupFaultObservation, AdaptiveLayerSnapshot,
    AdaptivePlanningDecisionKind, AdaptiveRunLimits, AdaptiveRunPhaseView, AdaptiveRunSnapshot,
    AdaptiveStopReasonView, AuthorizedSessionResume, BOUNDED_DELEGATION_REPORT_INSTRUCTION_V1,
    BoundedFlowResult, BoundedHelperResult, BoundedHelperResultStatus, BoundedHelperRunOutcome,
    BoundedMemberRunError, BoundedResultSpec, BoundedTurnFailure, BoundedTurnResult,
    BoundedTurnWaitError, ControllingAcceptorConfig, CurrentMobAdmission,
    DEFAULT_BOUNDED_HELPER_RESULT_BYTES, DelegationCancellationHandle, DelegationExecutionError,
    DelegationExecutionHandle, DelegationExecutionOutcome, DelegationExecutionRequest,
    DelegationExecutionService, DelegationExecutionSource, DelegationMemberOptions,
    DelegationParentContext, DelegationTerminalizedExecution, DelegationTurnTerminal,
    DurableBoundedMemberState, DurableBoundedWorkRecovery, DurableBoundedWorkState,
    ExternalPeerBindingSpec, FlowRunHandle, FlowRunWaitError, FlowTargetProvisioner,
    ForkMemberBoundedRunOutcome, ForkMemberResult, HELPER_RESULT_TRUNCATION_MARKER, HelperOptions,
    HelperResult, HostBindReport, HostBindRequest, HostCapabilityReport, HostRevokeReport,
    IdentityLocalExternalToolsError, IdentityLocalExternalToolsProvider,
    IdentityLocalMaterializationKey, InitializeAdaptiveRunRequest, LiveDelegationTerminalEvidence,
    MemberBoundedTurnResult, MemberDeliveryReceipt, MemberHandle, MemberHistoryPageDomain,
    MemberLiveStatusDomain, MemberRespawnReceipt, MemberTurnEventSender, MemberTurnHandle,
    MemberTurnOptions, MobBuilder, MobDestroyError, MobDestroyReport, MobEventRouterConfig,
    MobEventRouterHandle, MobEventsSubscription, MobEventsSubscriptionConfig, MobHandle,
    MobMachineStateChanges, MobMemberSnapshot, MobMemberStatus, MobPeerConnectivitySnapshot,
    MobRespawnError, MobSessionService, MobSpawnManyFailure, MobState, MobUnreachablePeer,
    MobWireMembersBatchReport, PeerMessageReceipt, PeerTarget, PreviousMemberCleanupReport,
    ResumeRejectionKind, ResumeSessionLoad, ResumeVerdictTerminality, SessionResumeAuthority,
    SessionResumeLifecycle, SessionResumeMaterialization, SessionResumePreparationReceipt,
    SessionResumeRejection, SessionResumeVerdict, SpawnContinuityIntent, SpawnCustomizationContext,
    SpawnMemberAdmission, SpawnMemberAdmissionObservations, SpawnMemberCustomizer, SpawnMemberSpec,
    SpawnPolicy, SpawnResult, SpawnSource, SpawnSpec, SpawnSystemPromptOverride,
    SpawnToolAdmission, SupervisorRotationReport, WorkBoundedTurnResult, WorkDeliveryReceipt,
    WorkTurnHandle, materialize_nonpersistent_session_resume_verdict, mob_error_wire_code,
    profile_to_wire, render_bounded_delegation_task, stored_realm_profile_to_wire,
};
#[cfg(feature = "experimental-gpt-live")]
pub use runtime::{
    DEFAULT_LIVE_BRIDGE_OUTPUT_BYTES, DurableMemberLiveBridgeOperationExecutor,
    LiveBridgeAcceptedExecution, LiveBridgeExecutionSnapshot,
    LiveBridgeOperationCancellationHandle, LiveBridgeOperationCancellationSignal,
    LiveBridgeOperationExecutor, LiveBridgeOperationRequest, LiveBridgeOperationService,
    LiveBridgeOperationStartError, LiveBridgeOperationTerminal, LiveBridgeOperationTerminalError,
    LiveBridgeOperationTerminalFuture,
};
pub use runtime::{FlowFrameKernel, FlowFrameMutator};
pub use runtime::{
    FlowTurnExecutor, FlowTurnFailureDisposition, FlowTurnOutcome, FlowTurnTicket,
    TimeoutDisposition,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use runtime::{IdentityRecoveryFailStopPoint, arm_identity_recovery_fail_stop_for_test};
pub use runtime::{MobpackCallableConfig, MobpackRunOutcome, MobpackRunSpec};
pub use runtime::{SpawnBasePromptSource, StaticSpawnBasePromptSource};
pub use runtime_mode::MobRuntimeMode;
pub use spec::SpecValidator;
pub use storage::MobStorage;
pub use store::{
    ExternalBindingOverlayRecord, ExternalBindingOverlayStatus, InMemoryMobEventStore,
    InMemoryMobIdentityStatusStore, InMemoryMobIdentityStore, InMemoryMobRunStore,
    InMemoryMobRuntimeMetadataStore, InMemoryMobSpecStore, InMemoryRealmProfileStore,
    MobDeliveryIdentity, MobEventReceiver, MobEventStore, MobExternalDeliveryAbandonOutcome,
    MobExternalDeliveryBeginOutcome, MobExternalDeliveryClaimOutcome,
    MobExternalDeliveryCompleteOutcome, MobExternalDeliveryIdentity, MobExternalDeliveryIntent,
    MobExternalDeliveryPhase, MobExternalDeliveryRecord, MobExternalDeliveryRepairOutcome,
    MobExternalDeliveryRepairState, MobExternalDeliveryTargetKind, MobExternalDeliveryTerminal,
    MobExternalFlowLaunchOutcome, MobHostAuthorityDeletionAuthority,
    MobHostAuthorityPersistenceAuthority, MobHostAuthorityRecord, MobHostBindPhaseRecord,
    MobHostCapabilityRecord, MobIdentityStatusStore, MobIdentityStore, MobIdentityStoreClock,
    MobOperatorGrantDeletionAuthority, MobOperatorGrantPersistenceAuthority,
    MobOperatorGrantRecord, MobRunStore, MobRuntimeMetadataStore, MobSpecStore, MobStoreError,
    RealmProfileStore, StoredRealmProfile, SupervisorAuthorityRecord, SystemMobIdentityStoreClock,
};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    SqliteMobEventStore, SqliteMobRunStore, SqliteMobRuntimeMetadataStore, SqliteMobSpecStore,
    SqliteMobStores, SqliteRealmProfileStore,
};
pub use validate::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, partition_diagnostics, validate_definition,
};

/// Closure called at each member spawn to get a fresh snapshot of external tools.
///
/// Returns `None` when no external tools are registered yet (e.g. before SDK
/// has called `tools/register`). The mob layer calls this lazily per-spawn so
/// tools registered after mob creation are picked up.
pub type ExternalToolsProvider = std::sync::Arc<
    dyn Fn() -> Option<std::sync::Arc<dyn meerkat_core::agent::AgentToolDispatcher>> + Send + Sync,
>;

#[cfg(test)]
mod tests;
