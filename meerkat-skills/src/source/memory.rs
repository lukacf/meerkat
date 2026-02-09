//! In-memory skill source for testing.

use async_trait::async_trait;
use meerkat_core::skills::{SkillDescriptor, SkillDocument, SkillError, SkillId, SkillSource};

/// In-memory skill source for testing and SDK embedding.
pub struct InMemorySkillSource {
    skills: Vec<SkillDocument>,
}

impl InMemorySkillSource {
    pub fn new(skills: Vec<SkillDocument>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl SkillSource for InMemorySkillSource {
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        Ok(self.skills.iter().map(|s| s.descriptor.clone()).collect())
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        self.skills
            .iter()
            .find(|s| &s.descriptor.id == id)
            .cloned()
            .ok_or_else(|| SkillError::NotFound { id: id.clone() })
    }
}
