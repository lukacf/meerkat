//! meerkat-session — Session service orchestration for Meerkat.
//!
//! This crate provides `EphemeralSessionService` (always available) and,
//! behind feature gates, `PersistentSessionService` and `DefaultCompactor`.
//!
//! # Features
//!
//! - `session-store`: Enables `PersistentSessionService`.
//! - `session-compaction`: Enables `DefaultCompactor` and `CompactionConfig`.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod ephemeral;
pub(crate) mod generated;
pub mod maintenance;
pub mod staged_registry;
pub(crate) mod turn_admission;

#[cfg(feature = "session-compaction")]
pub mod compactor;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod event_store;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod persistent;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub mod projector;

pub use ephemeral::{
    EphemeralSessionService, LiveSessionActorRegistry, LiveSessionActorWitness,
    LiveSessionActorWitnessSlot, RuntimeContextAdmissionGuard, SessionAgent, SessionAgentBuilder,
    SessionSnapshot,
};
pub use staged_registry::{AdmissionOutcome, MaterializationStatus, StagedSessionRegistry};

/// Metadata key used to store session labels in the `Session.metadata` map.
///
/// This is a persistence implementation detail — the ephemeral service stores
/// labels directly on its in-memory handle; the persistent service reads/writes
/// this key in the session snapshot for durable storage.
pub const SESSION_LABELS_KEY: &str = "session_labels";

/// Type alias for the raw broadcast receiver used by event subscriptions.
/// Exported for WASM surface which needs synchronous `try_recv()`.
pub type BroadcastEventReceiver =
    tokio::sync::broadcast::Receiver<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>;

#[cfg(feature = "session-compaction")]
pub use compactor::DefaultCompactor;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use persistent::{
    LegacyCheckpointAdoptionOptions, LegacyCheckpointAdoptionReport,
    LiveSessionActorTurnBoundaryLease, MachineServiceTurnCommitProtocol,
    MachineSessionArchiveProtocol, PersistentSessionService,
};

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
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::SessionStore,
        description: "PersistentSessionService, SessionProjector",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: Some("session-store"),
        prerequisites: &[],
        status_resolver: None,
    }
}

#[cfg(all(feature = "session-compaction", not(target_arch = "wasm32")))]
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::SessionCompaction,
        description: "DefaultCompactor: auto-compact at token threshold, LLM summary, history rebuild",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: Some("session-compaction"),
        prerequisites: &[],
        status_resolver: None,
    }
}

/// Convert a [`meerkat_core::service::SessionControlError`] into the public
/// [`meerkat_core::service::SessionError`] surface: session-tier causes pass
/// through unchanged; control-tier causes surface as typed `Unsupported`.
///
/// Single shared owner for the ephemeral and persistent services (the
/// persistent twin previously carried a private copy).
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub(crate) fn control_error_into_session_error(
    err: meerkat_core::service::SessionControlError,
) -> meerkat_core::service::SessionError {
    match err {
        meerkat_core::service::SessionControlError::Session(session_err) => session_err,
        other => meerkat_core::service::SessionError::Unsupported(other.to_string()),
    }
}
