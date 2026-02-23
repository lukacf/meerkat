//! Meerkat Mob - Multi-agent orchestration runtime.
//!
//! This crate provides the runtime for orchestrating multiple Meerkat agents
//! (meerkats) as a collaborative mob. It handles spawning, wiring, lifecycle
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
//! - [`Roster`] - Projected view of active meerkats
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

pub mod backend;
pub mod build;
pub mod definition;
pub mod error;
pub mod event;
pub mod ids;
pub mod prefab;
pub mod profile;
pub mod roster;
pub mod run;
pub mod runtime;
pub mod runtime_mode;
pub mod spec;
pub mod storage;
pub mod store;
pub mod tasks;
pub mod validate;

// Re-exports for convenience
pub use backend::MobBackendKind;
pub use definition::MobDefinition;
pub use error::MobError;
pub use event::{
    MemberRef, MobEvent, MobEventCompat, MobEventCompatError, MobEventKind, MobEventKindCompat,
    NewMobEvent,
};
pub use ids::{BranchId, FlowId, MeerkatId, MobId, ProfileName, RunId, StepId, TaskId};
pub use prefab::Prefab;
pub use profile::{Profile, ToolConfig};
pub use roster::{MemberState, Roster, RosterEntry};
pub use run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, MobRun, MobRunStatus, StepLedgerEntry,
    StepRunStatus,
};
pub use runtime::{FlowTurnExecutor, FlowTurnOutcome, FlowTurnTicket, TimeoutDisposition};
pub use runtime::{MobBuilder, MobHandle, MobSessionService, MobState, SpawnMemberSpec};
pub use runtime_mode::MobRuntimeMode;
pub use spec::SpecValidator;
pub use storage::MobStorage;
pub use store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobEventStore, MobRunStore,
    MobSpecStore, RedbMobEventStore, RedbMobRunStore, RedbMobSpecStore, RedbMobStores,
};
pub use tasks::{MobTask, TaskBoard, TaskStatus};
pub use validate::{
    Diagnostic, DiagnosticCode, DiagnosticSeverity, partition_diagnostics, validate_definition,
};

// Embedded skill: mob communication instructions.
//
// Registered via inventory so the skill engine loads it as an embedded
// skill. Mob agents preload this via `preload_skills` in their build
// config, which means it survives any per-request system_prompt
// override (skills are appended as extra_sections, not baked into the
// base prompt).
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "mob-communication",
        name: "Mob Communication",
        description: "How to communicate with peers in a collaborative mob",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["comms"],
        body: include_str!("../skills/mob-communication/SKILL.md"),
        extensions: &[],
    }
}

#[cfg(test)]
mod tests;
