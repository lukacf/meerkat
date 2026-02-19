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

pub mod backend;
pub mod build;
pub mod definition;
pub mod error;
pub mod event;
pub mod ids;
pub mod prefab;
pub mod profile;
pub mod roster;
pub mod runtime;
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
pub use ids::{MeerkatId, MobId, ProfileName};
pub use prefab::Prefab;
pub use profile::{Profile, ToolConfig};
pub use roster::{Roster, RosterEntry};
pub use runtime::{MobBuilder, MobHandle, MobSessionService, MobState};
pub use storage::MobStorage;
pub use store::{InMemoryMobEventStore, MobEventStore};
pub use tasks::{MobTask, TaskBoard, TaskStatus};
pub use validate::{Diagnostic, DiagnosticCode, validate_definition};

#[cfg(test)]
mod tests;
