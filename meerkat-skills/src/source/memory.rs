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
    use meerkat_core::skills::{
        SkillKeyRemap, SkillName, SkillScope, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind,
        SourceUuid,
    };

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

    fn make_skill_with_key(key: SkillKey, name: &str) -> SkillDocument {
        SkillDocument {
            descriptor: SkillDescriptor {
                key,
                name: name.into(),
                description: format!("Desc for {name}"),
                scope: SkillScope::Builtin,
                metadata: IndexMap::new(),
                capability_requirements: Vec::new(),
                source_name: String::new(),
            },
            body: format!("Body for {name}"),
            extensions: IndexMap::new(),
        }
    }

    fn source_record(source_uuid: SourceUuid, name: &str) -> SourceIdentityRecord {
        SourceIdentityRecord {
            source_uuid,
            display_name: name.to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: format!("fixture:{name}"),
            status: SourceIdentityStatus::Active,
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

    #[tokio::test]
    async fn test_load_with_source_identity_registry_applies_remap() {
        let legacy_source = SourceUuid::parse("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").unwrap();
        let canonical_source = SourceUuid::parse("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb").unwrap();
        let skill_name = SkillName::parse("email").unwrap();
        let legacy_key = SkillKey::new(legacy_source.clone(), skill_name.clone());
        let canonical_key = SkillKey::new(canonical_source.clone(), skill_name);
        let registry = SourceIdentityRegistry::build(
            vec![
                source_record(legacy_source.clone(), "legacy"),
                source_record(canonical_source.clone(), "canonical"),
            ],
            vec![SourceIdentityLineage {
                event_id: "legacy-to-canonical".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: Vec::new(),
                event: SourceIdentityLineageEvent::RenameOrRelocate {
                    from: legacy_source,
                    to: canonical_source,
                },
            }],
            vec![SkillKeyRemap {
                from: legacy_key.clone(),
                to: canonical_key.clone(),
                reason: Some("test remap".to_string()),
            }],
            Vec::new(),
        )
        .unwrap();
        let source = InMemorySkillSource::new(vec![make_skill_with_key(
            canonical_key.clone(),
            "canonical-email",
        )]);

        assert!(matches!(
            source.load(&legacy_key).await,
            Err(SkillError::NotFound { .. })
        ));

        let result = source
            .load_with_source_identity_registry(&legacy_key, &registry)
            .await
            .unwrap();
        assert_eq!(result.descriptor.key, canonical_key);
        assert_eq!(result.descriptor.name, "canonical-email");
    }
}
