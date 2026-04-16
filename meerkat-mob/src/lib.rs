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
//! - [`TaskBoard`] - Projected view of shared tasks
//! - [`MobEventStore`] - Persistence trait for mob events
//! - [`MobStorage`] - Storage bundle for a mob
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::redundant_clone,
        clippy::io_other_error,
        clippy::collapsible_if,
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
pub mod definition;
pub mod error;
pub mod event;
mod generated;
pub mod ids;
pub mod launch;
pub(crate) mod machines;
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
pub mod tasks;
pub mod validate;

// Re-exports for convenience
pub use backend::{MobBackendKind, RuntimeBinding};
pub use definition::MobDefinition;
pub use error::MobError;
pub use event::{AttributedEvent, MobEvent, MobEventKind, NewMobEvent};
pub use ids::{
    AgentIdentity, AgentRuntimeId, BranchId, FenceToken, FlowId, FlowNodeId, FrameId, Generation,
    LoopId, LoopInstanceId, MobId, ProfileName, RunId, StepId, TaskId, WorkOrigin, WorkRef,
    WorkSpec,
};
pub use launch::{BudgetSplitPolicy, ForkContext};
#[doc(hidden)]
pub use mob_machine::canonical_mob_machine_command_manifest;
pub use profile::{Profile, ProfileBinding, ProfileSource, SpawnTooling, ToolConfig};
pub use roster::{MemberState, MobMemberKickoffPhase, MobMemberKickoffSnapshot};
pub use run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, FrameSnapshot, LoopContextHistory,
    LoopIterationLedgerEntry, LoopSnapshot, MobRun, MobRunStatus, StepLedgerEntry, StepRunStatus,
};
pub use runtime::RestoreIncompatible;
pub use runtime::{FlowFrameKernel, FlowFrameMutator};
pub use runtime::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};
pub use runtime::{
    HelperOptions, HelperResult, MemberDeliveryReceipt, MemberHandle, MemberRespawnReceipt,
    MobBuilder, MobEventRouterConfig, MobEventRouterHandle, MobHandle, MobMemberSnapshot,
    MobMemberStatus, MobPeerConnectivitySnapshot, MobRespawnError, MobSessionService, MobState,
    MobUnreachablePeer, PeerTarget, SpawnMemberSpec, SpawnPolicy, SpawnResult, SpawnSpec,
};
pub use runtime::{SchedulerGrant, pump_schedulers_to_exhaustion};
pub use runtime_mode::MobRuntimeMode;
pub use snapshot::ParentToolScopeSnapshot;
pub use spec::SpecValidator;
pub use storage::MobStorage;
pub use store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, InMemoryRealmProfileStore,
    MobEventStore, MobRunStore, MobSpecStore, MobStoreError, RealmProfileStore, StoredRealmProfile,
};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    SqliteMobEventStore, SqliteMobRunStore, SqliteMobSpecStore, SqliteMobStores,
    SqliteRealmProfileStore,
};
pub use tasks::{MobTask, TaskBoard, TaskStatus};
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
