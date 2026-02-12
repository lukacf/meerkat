//! In-memory skill source for testing.

use async_trait::async_trait;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillSource, apply_filter,
};

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
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let all: Vec<SkillDescriptor> = self.skills.iter().map(|s| s.descriptor.clone()).collect();
        Ok(apply_filter(&all, filter))
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        self.skills
            .iter()
            .find(|s| &s.descriptor.id == id)
            .cloned()
            .ok_or_else(|| SkillError::NotFound { id: id.clone() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use meerkat_core::skills::SkillScope;

    fn make_skill(id: &str, name: &str) -> SkillDocument {
        SkillDocument {
            descriptor: SkillDescriptor {
                id: SkillId(id.into()),
                name: name.into(),
                description: format!("Desc for {name}"),
                scope: SkillScope::Builtin,
                ..Default::default()
            },
            body: format!("Body for {id}"),
            extensions: IndexMap::new(),
        }
    }

    fn test_source() -> InMemorySkillSource {
        InMemorySkillSource::new(vec![
            make_skill("extraction/email", "email"),
            make_skill("extraction/fiction", "fiction"),
            make_skill("formatting/markdown", "markdown"),
            make_skill("pdf-processing", "pdf-processing"),
        ])
    }

    #[tokio::test]
    async fn test_list_with_empty_filter() {
        let source = test_source();
        let result = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn test_list_with_collection_filter() {
        let source = test_source();
        let result = source
            .list(&SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|s| s.id.0.starts_with("extraction/")));
    }

    #[tokio::test]
    async fn test_list_collection_filter_no_sibling() {
        let source = InMemorySkillSource::new(vec![
            make_skill("extraction/email", "email"),
            make_skill("extract/something", "something"),
        ]);
        let result = source
            .list(&SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id.0, "extraction/email");
    }
}
