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
pub mod resolve;
pub mod resolver;
pub mod source;

pub use engine::DefaultSkillEngine;
pub use registration::{SkillRegistration, collect_registered_skills};
#[allow(deprecated)] // deprecated ambient wrapper stays exported through its window
pub use resolve::{resolve_repositories, resolve_repositories_with_roots};
#[cfg(not(target_arch = "wasm32"))]
pub use source::FilesystemSkillSource;
pub use source::{CompositeSkillSource, EmbeddedSkillSource, InMemorySkillSource, NamedSource};

pub const SKILLS_CAPABILITY_DISABLED_DESCRIPTION: &str = "config.skills.enabled is false";

pub fn skills_capability_enabled(config: &meerkat_core::Config) -> bool {
    config.skills.enabled
}

pub const SKILLS_CAPABILITY_POLICY: meerkat_capabilities::FeatureCapabilityPolicy =
    meerkat_capabilities::FeatureCapabilityPolicy::new(
        skills_capability_enabled,
        SKILLS_CAPABILITY_DISABLED_DESCRIPTION,
    );

pub const fn skills_capability_policy() -> meerkat_capabilities::FeatureCapabilityPolicy {
    SKILLS_CAPABILITY_POLICY
}

#[cfg(feature = "capability")]
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Skills,
        description: "Skill loading, resolution, and injection",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: Some("skills"),
        prerequisites: &[],
        status_resolver: Some(|config| {
            let policy = crate::skills_capability_policy();
            if policy.is_enabled(config) {
                meerkat_capabilities::CapabilityStatus::Available
            } else {
                meerkat_capabilities::CapabilityStatus::DisabledByPolicy {
                    description: policy.disabled_description().into(),
                }
            }
        }),
    }
}
