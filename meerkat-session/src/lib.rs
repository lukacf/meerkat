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

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod ephemeral;

#[cfg(feature = "session-compaction")]
pub mod compactor;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod event_store;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod persistent;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod projector;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod redb_events;

pub use ephemeral::{EphemeralSessionService, SessionAgent, SessionAgentBuilder, SessionSnapshot};

/// Type alias for the raw broadcast receiver used by event subscriptions.
/// Exported for WASM surface which needs synchronous `try_recv()`.
pub type BroadcastEventReceiver = tokio::sync::broadcast::Receiver<meerkat_core::AgentEvent>;

#[cfg(feature = "session-compaction")]
pub use compactor::DefaultCompactor;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use persistent::PersistentSessionService;

// Skill registration (inventory + meerkat-skills not available on wasm32)
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
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

// Capability registrations (inventory not available on wasm32)
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
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

#[cfg(all(feature = "session-compaction", not(target_arch = "wasm32")))]
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
