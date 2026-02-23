//! Composite skill source that merges multiple named sources with precedence.

use crate::source::SourceNode;
use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillDescriptor, SkillDocument, SkillError, SkillFilter,
    SkillId, SkillIntrospectionEntry, SkillQuarantineDiagnostic, SkillSource, SourceHealthSnapshot,
    SourceHealthState,
};
use std::collections::HashMap;

/// A named skill source for provenance tracking.
pub struct NamedSource {
    pub name: String,
    pub source: SourceNode,
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
    pub fn new(sources: Vec<SourceNode>) -> Self {
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

impl SkillSource for CompositeSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
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
        for named in &self.sources {
            match named.source.load(id).await {
                Ok(doc) => return Ok(doc),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }

    async fn quarantined_diagnostics(&self) -> Result<Vec<SkillQuarantineDiagnostic>, SkillError> {
        let mut all = Vec::new();
        for named in &self.sources {
            let mut diagnostics = named.source.quarantined_diagnostics().await?;
            for diag in &mut diagnostics {
                diag.location = format!("{}:{}", named.name, diag.location);
            }
            all.extend(diagnostics);
        }
        Ok(all)
    }

    async fn health_snapshot(&self) -> Result<SourceHealthSnapshot, SkillError> {
        let mut aggregate = SourceHealthSnapshot::default();
        for named in &self.sources {
            let snapshot = named.source.health_snapshot().await?;
            aggregate.invalid_count = aggregate
                .invalid_count
                .saturating_add(snapshot.invalid_count);
            aggregate.total_count = aggregate.total_count.saturating_add(snapshot.total_count);
            aggregate.failure_streak = aggregate.failure_streak.max(snapshot.failure_streak);
            aggregate.handshake_failed |= snapshot.handshake_failed;
            aggregate.state = match (aggregate.state, snapshot.state) {
                (SourceHealthState::Unhealthy, _) | (_, SourceHealthState::Unhealthy) => {
                    SourceHealthState::Unhealthy
                }
                (SourceHealthState::Degraded, _) | (_, SourceHealthState::Degraded) => {
                    SourceHealthState::Degraded
                }
                _ => SourceHealthState::Healthy,
            };
        }
        if aggregate.total_count > 0 {
            aggregate.invalid_ratio = aggregate.invalid_count as f32 / aggregate.total_count as f32;
        }
        Ok(aggregate)
    }

    async fn list_artifacts(&self, id: &SkillId) -> Result<Vec<SkillArtifact>, SkillError> {
        for named in &self.sources {
            match named.source.list_artifacts(id).await {
                Ok(artifacts) => return Ok(artifacts),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }

    async fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        for named in &self.sources {
            match named.source.read_artifact(id, artifact_path).await {
                Ok(artifact) => return Ok(artifact),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }

    async fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        for named in &self.sources {
            match named
                .source
                .invoke_function(id, function_name, arguments.clone())
                .await
            {
                Ok(output) => return Ok(output),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { id: id.clone() })
    }

    async fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillIntrospectionEntry>, SkillError> {
        // Track which source won each skill ID.
        let mut first_source: HashMap<SkillId, String> = HashMap::new();
        let mut result = Vec::new();

        for named in &self.sources {
            let descriptors = named.source.list(filter).await?;
            for mut desc in descriptors {
                if let Some(winning_source) = first_source.get(&desc.id) {
                    // This skill is shadowed by a higher-precedence source.
                    desc.source_name.clone_from(&named.name);
                    result.push(SkillIntrospectionEntry {
                        descriptor: desc,
                        shadowed_by: Some(winning_source.clone()),
                        is_active: false,
                    });
                } else {
                    desc.source_name.clone_from(&named.name);
                    first_source.insert(desc.id.clone(), named.name.clone());
                    result.push(SkillIntrospectionEntry {
                        descriptor: desc,
                        shadowed_by: None,
                        is_active: true,
                    });
                }
            }
        }
        Ok(result)
    }

    async fn load_from_source(
        &self,
        id: &SkillId,
        source_name: Option<&str>,
    ) -> Result<SkillDocument, SkillError> {
        match source_name {
            Some(name) => {
                // Load from a specific named source, bypassing first-wins.
                for named in &self.sources {
                    if named.name == name {
                        return named.source.load(id).await;
                    }
                }
                Err(SkillError::NotFound { id: id.clone() })
            }
            None => {
                // Normal first-wins resolution.
                self.load(id).await
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::source::InMemorySkillSource;
    use crate::source::SourceNode;
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
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: SourceNode::Memory(source_b),
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
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: SourceNode::Memory(source_b),
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
        let source_b =
            InMemorySkillSource::new(vec![make_skill("formatting/markdown", "markdown")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "a".into(),
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "b".into(),
                source: SourceNode::Memory(source_b),
            },
        ]);

        let skills = composite.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 3);
    }

    #[tokio::test]
    async fn test_list_all_with_provenance_returns_active_and_shadowed() {
        let source_a = InMemorySkillSource::new(vec![make_skill("shared/skill", "skill-a")]);
        let source_b = InMemorySkillSource::new(vec![make_skill("shared/skill", "skill-b")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "project".into(),
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: SourceNode::Memory(source_b),
            },
        ]);

        let entries = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);

        let active = entries.iter().find(|e| e.is_active).unwrap();
        assert_eq!(active.descriptor.source_name, "project");
        assert!(active.shadowed_by.is_none());

        let shadowed = entries.iter().find(|e| !e.is_active).unwrap();
        assert_eq!(shadowed.descriptor.source_name, "company");
        assert_eq!(shadowed.shadowed_by.as_deref(), Some("project"));
    }

    #[tokio::test]
    async fn test_list_all_with_provenance_non_overlapping_all_active() {
        let source_a = InMemorySkillSource::new(vec![make_skill("a/skill-1", "skill-1")]);
        let source_b = InMemorySkillSource::new(vec![make_skill("b/skill-2", "skill-2")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "project".into(),
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: SourceNode::Memory(source_b),
            },
        ]);

        let entries = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.is_active));
        assert!(entries.iter().all(|e| e.shadowed_by.is_none()));
    }

    #[tokio::test]
    async fn test_load_from_source_bypasses_first_wins() {
        let source_a = InMemorySkillSource::new(vec![make_skill("shared/skill", "skill-a")]);
        let source_b = InMemorySkillSource::new(vec![make_skill("shared/skill", "skill-b")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "project".into(),
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "company".into(),
                source: SourceNode::Memory(source_b),
            },
        ]);

        // Normal load returns project (first wins)
        let normal = composite
            .load(&SkillId("shared/skill".into()))
            .await
            .unwrap();
        assert_eq!(normal.descriptor.name, "skill-a");

        // load_from_source can access the shadowed company source
        let from_company = composite
            .load_from_source(&SkillId("shared/skill".into()), Some("company"))
            .await
            .unwrap();
        assert_eq!(from_company.descriptor.name, "skill-b");
    }

    #[tokio::test]
    async fn test_load_from_source_none_uses_normal_resolution() {
        let source_a = InMemorySkillSource::new(vec![make_skill("a/skill", "skill-a")]);

        let composite = CompositeSkillSource::from_named(vec![NamedSource {
            name: "project".into(),
            source: SourceNode::Memory(source_a),
        }]);

        let doc = composite
            .load_from_source(&SkillId("a/skill".into()), None)
            .await
            .unwrap();
        assert_eq!(doc.descriptor.name, "skill-a");
    }

    #[tokio::test]
    async fn test_collections_merged_across_sources() {
        let source_a = InMemorySkillSource::new(vec![make_skill("extraction/email", "email")]);
        let source_b = InMemorySkillSource::new(vec![make_skill("extraction/fiction", "fiction")]);

        let composite = CompositeSkillSource::from_named(vec![
            NamedSource {
                name: "a".into(),
                source: SourceNode::Memory(source_a),
            },
            NamedSource {
                name: "b".into(),
                source: SourceNode::Memory(source_b),
            },
        ]);

        let collections = composite.collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(collections[0].count, 2); // both skills count
    }
}
