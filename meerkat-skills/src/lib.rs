//! meerkat-skills — Skill loading, resolution, and injection for Meerkat.
//!
//! Provides `DefaultSkillEngine`, skill sources (filesystem, embedded, memory),
//! frontmatter parsing, reference resolution, and content rendering.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod engine;
pub mod parser;
pub mod registration;
pub mod renderer;
#[cfg(not(target_arch = "wasm32"))]
pub mod resolve;
pub mod resolver;
pub mod source;

pub use engine::DefaultSkillEngine;
pub use registration::{SkillRegistration, collect_registered_skills};
#[cfg(not(target_arch = "wasm32"))]
pub use resolve::{resolve_repositories, resolve_repositories_with_roots};
pub use source::{CompositeSkillSource, EmbeddedSkillSource, InMemorySkillSource, NamedSource};
#[cfg(not(target_arch = "wasm32"))]
pub use source::FilesystemSkillSource;

// Capability registration
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::Skills,
        description: "Skill loading, resolution, and injection",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("skills"),
        prerequisites: &[],
        status_resolver: None,
    }
}
