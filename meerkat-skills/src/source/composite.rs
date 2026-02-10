//! Composite skill source that merges multiple sources with precedence.

use async_trait::async_trait;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillSource,
};
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
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for source in &self.sources {
            let descriptors = source.list(filter).await?;
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
        let empty_filter = SkillFilter::default();
        for source in &self.sources {
            let descriptors = source.list(&empty_filter).await?;
            if descriptors.iter().any(|d| &d.id == id) {
                return source.load(id).await;
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }
}
