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

// Skill registration
#[cfg(feature = "session-store")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "session-management",
        name: "Session Management",
        description: "Session persistence, resume patterns, event store replay, compaction tuning",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["session_store"],
        body: include_str!("../skills/session-management/SKILL.md"),
        extensions: &[],
    }
}

// Capability registrations
#[cfg(feature = "session-store")]
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::SessionStore,
        description: "PersistentSessionService, RedbEventStore, SessionProjector",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("session-store"),
        prerequisites: &[],
        status_resolver: None,
    }
}

#[cfg(feature = "session-compaction")]
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::SessionCompaction,
        description: "DefaultCompactor: auto-compact at token threshold, LLM summary, history rebuild",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("session-compaction"),
        prerequisites: &[],
        status_resolver: None,
    }
}
