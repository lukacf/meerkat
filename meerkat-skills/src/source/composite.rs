//! Composite skill source that merges multiple sources with precedence.

use async_trait::async_trait;
use meerkat_core::skills::{SkillDescriptor, SkillDocument, SkillError, SkillId, SkillSource};
use std::collections::HashSet;

/// Merges skills from multiple sources with precedence (first source wins for duplicate IDs).
pub struct CompositeSkillSource {
    sources: Vec<Box<dyn SkillSource>>,
}

impl CompositeSkillSource {
    pub fn new(sources: Vec<Box<dyn SkillSource>>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl SkillSource for CompositeSkillSource {
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for source in &self.sources {
            let descriptors = source.list().await?;
            for desc in descriptors {
                if seen.insert(desc.id.clone()) {
                    result.push(desc);
                } else {
                    tracing::debug!("Skill {} shadowed by higher-precedence source", desc.id);
                }
            }
        }

        Ok(result)
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        for source in &self.sources {
            let descriptors = source.list().await?;
            if descriptors.iter().any(|d| &d.id == id) {
                return source.load(id).await;
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }
}
