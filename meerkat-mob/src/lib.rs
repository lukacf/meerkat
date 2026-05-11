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

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod backend;
mod build;
pub mod coordination;
pub mod definition;
pub mod error;
pub mod event;
#[doc(hidden)]
pub mod generated;
pub mod ids;
pub mod launch;
#[doc(hidden)]
pub mod machines;
mod mob_machine;
pub mod profile;
mod roster;
pub mod run;
pub mod runtime;
pub mod runtime_mode;
pub mod snapshot;
pub mod spec;
pub mod storage;
pub mod store;
pub mod validate;

// Re-exports for convenience
pub use backend::{MobBackendKind, RuntimeBinding};
pub use coordination::{
    CoordinationOwner, CoordinationRecordRefs, CoordinationResourceRef, MobCoordinationBoard,
    MobCoordinationError, MobCoordinationEvent, MobCoordinationEventKind, MobCoordinationSnapshot,
    NewResourceClaim, NewWorkIntent, ResourceClaim, ResourceClaimId, ResourceClaimKind,
    ResourceClaimStatus, WorkIntent, WorkIntentId, WorkIntentStatus,
};
pub use definition::MobDefinition;
pub use error::MobError;
pub use event::{AttributedEvent, MobEvent, MobEventKind, NewMobEvent};
pub use ids::{
    AgentIdentity, AgentRuntimeId, BranchId, FenceToken, FlowId, FlowNodeId, FrameId, Generation,
    LoopId, LoopInstanceId, MobId, ProfileName, RunId, StepId, WorkOrigin, WorkRef, WorkSpec,
};
pub use launch::{BudgetSplitPolicy, ForkContext, MemberLaunchMode};
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

#[doc(hidden)]
pub mod machine_schema_exports {
    pub fn mob_machine_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::mob_machine_schema_metadata()
            .attach_to(crate::machines::mob_machine::MobMachineState::schema())
    }
}

pub use profile::{Profile, ProfileBinding, ProfileSource, SpawnTooling, ToolConfig};
pub use roster::{MemberState, MobMemberKickoffPhase, MobMemberKickoffSnapshot};
pub use run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, FrameSnapshot, LoopContextHistory,
    LoopIterationLedgerEntry, LoopSnapshot, MobRun, MobRunStatus, StepLedgerEntry, StepRunStatus,
};
pub use runtime::RestoreIncompatible;
pub use runtime::bridge::{
    MobBoundMemberRuntimeBridge, MobMemberRuntimeBridge, observation_is_terminal,
};
pub use runtime::bridge_protocol::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeCapabilities, BridgeCommand,
    BridgeDeliveryOutcome, BridgeDeliveryPayload, BridgeDeliveryRejectionCause,
    BridgeDeliveryResponse, BridgeDestroyResponse, BridgeHardCancelPayload,
    BridgeMemberRuntimeState, BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec,
    BridgePeerWiringPayload, BridgeReply, BridgeRetireResponse, BridgeSupervisorPayload,
};
#[cfg(feature = "runtime-adapter")]
pub use runtime::local_bridge::LocalMobRuntimeBridge;
pub use runtime::{
    ExternalPeerBindingSpec, HelperOptions, HelperResult, MemberDeliveryReceipt, MemberHandle,
    MemberRespawnReceipt, MobBuilder, MobDestroyError, MobDestroyReport, MobEventRouterConfig,
    MobEventRouterHandle, MobEventsSubscription, MobEventsSubscriptionConfig, MobHandle,
    MobMemberSnapshot, MobMemberStatus, MobPeerConnectivitySnapshot, MobRespawnError,
    MobSessionService, MobState, MobUnreachablePeer, PeerTarget, PreviousMemberCleanupReport,
    SpawnMemberSpec, SpawnPolicy, SpawnResult, SpawnSpec, SupervisorRotationReport,
    WorkDeliveryReceipt,
};
pub use runtime::{FlowFrameKernel, FlowFrameMutator};
pub use runtime::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};
pub use runtime_mode::MobRuntimeMode;
pub use snapshot::ParentToolScopeSnapshot;
pub use spec::SpecValidator;
pub use storage::MobStorage;
pub use store::{
    ExternalBindingOverlayRecord, ExternalBindingOverlayStatus, InMemoryMobEventStore,
    InMemoryMobRunStore, InMemoryMobRuntimeMetadataStore, InMemoryMobSpecStore,
    InMemoryRealmProfileStore, MobEventReceiver, MobEventStore, MobRunStore,
    MobRuntimeMetadataStore, MobSpecStore, MobStoreError, RealmProfileStore, StoredRealmProfile,
    SupervisorAuthorityRecord,
};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    SqliteMobEventStore, SqliteMobRunStore, SqliteMobRuntimeMetadataStore, SqliteMobSpecStore,
    SqliteMobStores, SqliteRealmProfileStore,
};
pub use validate::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, partition_diagnostics, validate_definition,
};

pub(crate) use ids::MeerkatId;

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
