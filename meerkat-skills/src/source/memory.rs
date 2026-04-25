//! In-memory skill source for testing.

use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillSource, apply_filter,
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

impl SkillSource for InMemorySkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let all: Vec<SkillDescriptor> = self.skills.iter().map(|s| s.descriptor.clone()).collect();
        Ok(apply_filter(&all, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        self.skills
            .iter()
            .find(|s| &s.descriptor.key == key)
            .cloned()
            .ok_or_else(|| SkillError::NotFound { key: key.clone() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use meerkat_core::skills::{SkillName, SkillScope, SourceUuid};

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap(),
            skill_name: SkillName::parse(skill).unwrap(),
        }
    }

    fn make_skill(skill: &str, name: &str) -> SkillDocument {
        SkillDocument {
            descriptor: SkillDescriptor {
                key: test_key(skill),
                name: name.into(),
                description: format!("Desc for {name}"),
                scope: SkillScope::Builtin,
                metadata: IndexMap::new(),
                capability_requirements: Vec::new(),
                source_name: String::new(),
            },
            body: format!("Body for {skill}"),
            extensions: IndexMap::new(),
        }
    }

    fn test_source() -> InMemorySkillSource {
        InMemorySkillSource::new(vec![
            make_skill("email", "email"),
            make_skill("pdf", "pdf"),
            make_skill("markdown", "markdown"),
            make_skill("workflow", "workflow"),
        ])
    }

    #[tokio::test]
    async fn test_list_with_empty_filter() {
        let source = test_source();
        let result = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(result.len(), 4);
    }

    #[tokio::test]
    async fn test_load_returns_not_found_for_missing() {
        let source = InMemorySkillSource::new(vec![make_skill("email", "email")]);
        let result = source.load(&test_key("missing")).await;
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_load_returns_document_for_known() {
        let source = InMemorySkillSource::new(vec![make_skill("email", "email")]);
        let result = source.load(&test_key("email")).await.unwrap();
        assert_eq!(result.descriptor.name, "email");
    }
}
