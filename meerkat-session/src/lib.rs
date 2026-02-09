//! meerkat-session — Session service orchestration for Meerkat.
//!
//! This crate provides `EphemeralSessionService` (always available) and,
//! behind feature gates, `PersistentSessionService`, `DefaultCompactor`,
//! and `RedbEventStore`.
//!
//! # Features
//!
//! - `session-store`: Enables `PersistentSessionService` and `RedbEventStore`.
//! - `session-compaction`: Enables `DefaultCompactor` and `CompactionConfig`.

pub mod ephemeral;

#[cfg(feature = "session-compaction")]
pub mod compactor;

#[cfg(feature = "session-store")]
pub mod event_store;

#[cfg(feature = "session-store")]
pub mod persistent;

#[cfg(feature = "session-store")]
pub mod projector;

#[cfg(feature = "session-store")]
pub mod redb_events;

pub use ephemeral::{EphemeralSessionService, SessionAgent, SessionAgentBuilder, SessionSnapshot};

#[cfg(feature = "session-compaction")]
pub use compactor::DefaultCompactor;

#[cfg(feature = "session-store")]
pub use persistent::PersistentSessionService;
