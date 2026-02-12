//! Embedded skill source (from inventory registrations).

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillSource, apply_filter,
};

use crate::registration::{SkillRegistration, collect_registered_skills};

/// Convert a static `SkillRegistration` to a `SkillDescriptor`.
fn registration_to_descriptor(reg: &SkillRegistration) -> SkillDescriptor {
    SkillDescriptor {
        id: SkillId(reg.id.to_string()),
        name: reg.name.to_string(),
        description: reg.description.to_string(),
        scope: reg.scope,
        requires_capabilities: reg
            .requires_capabilities
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ..Default::default()
    }
}

/// Convert a static `SkillRegistration` to a `SkillDocument`.
fn registration_to_document(reg: &SkillRegistration) -> SkillDocument {
    SkillDocument {
        descriptor: registration_to_descriptor(reg),
        body: reg.body.to_string(),
        extensions: reg
            .extensions
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<IndexMap<_, _>>(),
    }
}

/// Skill source that reads from `inventory`-registered skills.
pub struct EmbeddedSkillSource;

impl EmbeddedSkillSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmbeddedSkillSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SkillSource for EmbeddedSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let all: Vec<SkillDescriptor> = collect_registered_skills()
            .into_iter()
            .map(registration_to_descriptor)
            .collect();
        Ok(apply_filter(&all, filter))
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        collect_registered_skills()
            .into_iter()
            .find(|r| r.id == id.0)
            .map(registration_to_document)
            .ok_or_else(|| SkillError::NotFound { id: id.clone() })
    }
}
