//! Machine-readable durability vocabulary for storage slots.
//!
//! Every piece of state a storage provider supplies is declared `Durable`,
//! `RebuildableCache`, or `Scratch`, together with how the slot actually
//! resolved. The classes are serializable so deployment tooling can act on
//! them (for example clone `Durable` domains only in state-generation
//! deploys). Fail-closed rule (enforced at composition): a `Durable` slot
//! resolving to a non-persistent store is a startup error unless the realm
//! explicitly declares that domain ephemeral — never a silent in-memory
//! fallback.

use serde::{Deserialize, Serialize};

/// What a storage domain's contents mean for durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityClass {
    /// Loss is data loss (sessions, runtime checkpoints, schedules,
    /// workgraph, blobs, artifacts, memory text).
    Durable,
    /// Derivable from durable state; loss costs a rebuild (indexes,
    /// projections, caches).
    RebuildableCache,
    /// Ephemeral by design.
    Scratch,
}

/// How a slot actually resolved at composition time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityResolution {
    /// Backed by persistent storage.
    Persistent,
    /// Non-persistent by an explicit declaration (memory backend, declared
    /// ephemeral domain, tests) — a legitimate configured choice.
    DeclaredEphemeral,
    /// Non-persistent without a declaration. Composition refuses this for
    /// `Durable` slots (the fail-closed rule).
    NonPersistent,
}

/// One storage slot's declaration: domain name, class, and resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurabilityDeclaration {
    /// Stable domain name ("sessions", "runtime", "schedule", "workgraph",
    /// "blobs", "artifacts", ...).
    pub domain: String,
    pub class: DurabilityClass,
    pub resolution: DurabilityResolution,
}

impl DurabilityDeclaration {
    pub fn durable(domain: &str, resolution: DurabilityResolution) -> Self {
        Self {
            domain: domain.to_string(),
            class: DurabilityClass::Durable,
            resolution,
        }
    }

    /// True when this declaration violates the fail-closed rule on its own
    /// (callers additionally consult the realm manifest's declared
    /// ephemeral domains before refusing).
    pub fn is_undeclared_nonpersistent_durable(&self) -> bool {
        self.class == DurabilityClass::Durable
            && self.resolution == DurabilityResolution::NonPersistent
    }
}
