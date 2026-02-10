//! meerkat-skills — Skill loading, resolution, and injection for Meerkat.
//!
//! Provides `DefaultSkillEngine`, skill sources (filesystem, embedded, memory),
//! frontmatter parsing, reference resolution, and content rendering.

pub mod config;
pub mod engine;
pub mod parser;
pub mod registration;
pub mod renderer;
pub mod resolver;
pub mod source;

pub use config::{SkillResolutionMode, SkillsConfig};
pub use engine::DefaultSkillEngine;
pub use registration::{SkillRegistration, collect_registered_skills};
pub use source::{
    CompositeSkillSource, EmbeddedSkillSource, FilesystemSkillSource, InMemorySkillSource,
};

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
