//! Composite skill source that merges multiple named sources with precedence.

use async_trait::async_trait;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillSource,
};
use std::collections::HashMap;

/// A named skill source for provenance tracking.
pub struct NamedSource {
    pub name: String,
    pub source: Box<dyn SkillSource>,
}

/// Merges skills from multiple named sources with precedence.
///
/// First source wins for duplicate IDs. Each skill's `source_name` field
/// is populated from the `NamedSource.name` that provided it.
/// Shadowing is logged at `info` level with both source names.
pub struct CompositeSkillSource {
    sources: Vec<NamedSource>,
}

impl CompositeSkillSource {
    /// Create from named sources (precedence = order).
    pub fn from_named(sources: Vec<NamedSource>) -> Self {
        Self { sources }
    }

    /// Create from unnamed sources (backward compatibility).
    /// Sources are named "source_0", "source_1", etc.
    pub fn new(sources: Vec<Box<dyn SkillSource>>) -> Self {
        let named = sources
            .into_iter()
            .enumerate()
            .map(|(i, source)| NamedSource {
                name: format!("source_{i}"),
                source,
            })
            .collect();
        Self { sources: named }
    }
}

#[async_trait]
impl SkillSource for CompositeSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        // Single map tracks both membership and provenance (no redundant HashSet).
        let mut first_source: HashMap<SkillId, String> = HashMap::new();
        let mut result = Vec::new();

        for named in &self.sources {
            let descriptors = named.source.list(filter).await?;
            for mut desc in descriptors {
                if !first_source.contains_key(&desc.id) {
                    desc.source_name.clone_from(&named.name);
                    first_source.insert(desc.id.clone(), named.name.clone());
                    result.push(desc);
                } else {
                    let winning_source = first_source
                        .get(&desc.id)
                        .map(|s| s.as_str())
                        .unwrap_or("unknown");
                    tracing::info!(
                        skill_id = %desc.id,
                        shadowed_source = %named.name,
                        winning_source = %winning_source,
                        "Skill shadowed by higher-precedence repository"
                    );
                }
            }
        }

        Ok(result)
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        // Try each source directly — avoids listing every source (expensive
        // for HTTP/git) just to check membership.
        for named in &self.sources {
            match named.source.load(id).await {
                Ok(doc) => return Ok(doc),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::source::InMemorySkillSource;
    use indexmap::IndexMap;
    use meerkat_core::skills::{SkillDocument, SkillScope};

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

    #[tokio::test]
    async fn test_named_sources_populate_source_name() {
        let source_a = InMemorySkillSource::new(vec![make_skill("a/skill-1", "skill-1")]);
        let source_b = InMemorySkillSource::new(vec![make_skill("b/skill-2", "skill-2")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "project".into(),
                source: Box::new(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: Box::new(source_b),
            },
        ]);

        let skills = composite.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 2);

        let s1 = skills.iter().find(|s| s.id.0 == "a/skill-1").unwrap();
        assert_eq!(s1.source_name, "project");

        let s2 = skills.iter().find(|s| s.id.0 == "b/skill-2").unwrap();
        assert_eq!(s2.source_name, "company");
    }

    #[tokio::test]
    async fn test_shadowing_by_name() {
        let source_a = InMemorySkillSource::new(vec![make_skill("shared/skill", "skill-a")]);
        let source_b = InMemorySkillSource::new(vec![make_skill("shared/skill", "skill-b")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "project".into(),
                source: Box::new(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: Box::new(source_b),
            },
        ]);

        let skills = composite.list(&SkillFilter::default()).await.unwrap();
        // First source wins
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "skill-a");
        assert_eq!(skills[0].source_name, "project");
    }

    #[tokio::test]
    async fn test_list_merges_across_sources() {
        let source_a = InMemorySkillSource::new(vec![
            make_skill("extraction/email", "email"),
            make_skill("extraction/fiction", "fiction"),
        ]);
        let source_b = InMemorySkillSource::new(vec![make_skill(
            "formatting/markdown",
            "markdown",
        )]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "a".into(),
                source: Box::new(source_a),
            },
            NamedSource {
                name: "b".into(),
                source: Box::new(source_b),
            },
        ]);

        let skills = composite.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 3);
    }

    #[tokio::test]
    async fn test_collections_merged_across_sources() {
        let source_a =
            InMemorySkillSource::new(vec![make_skill("extraction/email", "email")]);
        let source_b = InMemorySkillSource::new(vec![make_skill(
            "extraction/fiction",
            "fiction",
        )]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "a".into(),
                source: Box::new(source_a),
            },
            NamedSource {
                name: "b".into(),
                source: Box::new(source_b),
            },
        ]);

        let collections = composite.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(collections[0].count, 2); // both skills count
    }
}
