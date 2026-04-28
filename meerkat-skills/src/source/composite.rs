//! Composite skill source that merges multiple named sources with precedence.

use crate::source::SourceNode;
use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillDescriptor, SkillDocument, SkillError, SkillFilter,
    SkillIntrospectionEntry, SkillKey, SkillQuarantineDiagnostic, SkillSource,
    SourceHealthSnapshot, SourceHealthState, SourceIdentityRecord, SourceIdentityRegistry,
    SourceIdentityStatus, SourceTransportKind, SourceUuid, apply_filter,
};
use std::collections::HashMap;
use std::sync::Arc;

/// A named skill source for provenance tracking.
pub struct NamedSource {
    pub identity: SourceIdentityRecord,
    pub source: SourceNode,
}

impl NamedSource {
    pub fn new(identity: SourceIdentityRecord, source: SourceNode) -> Self {
        Self { identity, source }
    }

    fn display_name(&self) -> &str {
        &self.identity.display_name
    }

    fn source_uuid(&self) -> &SourceUuid {
        &self.identity.source_uuid
    }
}

/// Merges skills from multiple named sources with precedence.
///
/// First source wins for duplicate `SkillKey`s. Each skill's `source_name`
/// field is populated from the `NamedSource.name` that provided it.
pub struct CompositeSkillSource {
    sources: Vec<NamedSource>,
    registry: Option<Arc<SourceIdentityRegistry>>,
}

impl CompositeSkillSource {
    /// Create from named sources (precedence = order).
    pub fn from_named(sources: Vec<NamedSource>) -> Self {
        Self {
            sources,
            registry: None,
        }
    }

    /// Create from named sources with source-identity resolution applied
    /// before direct composite loads.
    pub fn from_named_with_registry(
        sources: Vec<NamedSource>,
        registry: Arc<SourceIdentityRegistry>,
    ) -> Self {
        Self {
            sources,
            registry: Some(registry),
        }
    }

    pub fn with_source_identity_registry(mut self, registry: Arc<SourceIdentityRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Create from unnamed sources (backward compatibility).
    pub fn new(sources: Vec<SourceNode>) -> Self {
        let named = sources
            .into_iter()
            .enumerate()
            .map(|(i, source)| NamedSource {
                identity: SourceIdentityRecord {
                    source_uuid: SourceUuid::builtin(),
                    display_name: format!("source_{i}"),
                    transport_kind: SourceTransportKind::Embedded,
                    fingerprint: format!("legacy:source_{i}"),
                    status: SourceIdentityStatus::Active,
                },
                source,
            })
            .collect();
        Self {
            sources: named,
            registry: None,
        }
    }

    fn resolve_key_for_load(&self, key: &SkillKey) -> Result<SkillKey, SkillError> {
        let Some(registry) = &self.registry else {
            return Ok(key.clone());
        };
        registry
            .resolve(key)
            .map(|resolved| resolved.key)
            .map_err(|e| {
                SkillError::Load(format!("source identity resolution failed for {key}: {e}").into())
            })
    }

    async fn load_canonical(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        for named in &self.sources {
            match named.source.load(key).await {
                Ok(mut doc) => {
                    doc.descriptor.source_name = named.display_name().to_string();
                    return Ok(doc);
                }
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { key: key.clone() })
    }
}

impl SkillSource for CompositeSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let mut merged: Vec<SkillDescriptor> = Vec::new();
        let mut seen: HashMap<SkillKey, usize> = HashMap::new();
        for named in &self.sources {
            let items = named.source.list(filter).await?;
            for mut desc in items {
                if seen.contains_key(&desc.key) {
                    continue;
                }
                seen.insert(desc.key.clone(), merged.len());
                desc.source_name = named.display_name().to_string();
                merged.push(desc);
            }
        }
        Ok(apply_filter(&merged, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        let canonical_key = self.resolve_key_for_load(key)?;
        self.load_canonical(&canonical_key).await
    }

    async fn quarantined_diagnostics(&self) -> Result<Vec<SkillQuarantineDiagnostic>, SkillError> {
        let mut all = Vec::new();
        for named in &self.sources {
            let mut diagnostics = named.source.quarantined_diagnostics().await?;
            for diag in &mut diagnostics {
                diag.location = format!("{}:{}", named.display_name(), diag.location);
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

    async fn list_artifacts(&self, key: &SkillKey) -> Result<Vec<SkillArtifact>, SkillError> {
        for named in &self.sources {
            match named.source.list_artifacts(key).await {
                Ok(artifacts) => return Ok(artifacts),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { key: key.clone() })
    }

    async fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        for named in &self.sources {
            match named.source.read_artifact(key, artifact_path).await {
                Ok(artifact) => return Ok(artifact),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { key: key.clone() })
    }

    async fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        for named in &self.sources {
            match named
                .source
                .invoke_function(key, function_name, arguments.clone())
                .await
            {
                Ok(output) => return Ok(output),
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { key: key.clone() })
    }

    async fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillIntrospectionEntry>, SkillError> {
        let mut entries: Vec<SkillIntrospectionEntry> = Vec::new();
        let mut active_by_key: HashMap<SkillKey, SourceIdentityRecord> = HashMap::new();

        for named in &self.sources {
            let items = named.source.list(filter).await?;
            for mut desc in items {
                desc.source_name = named.display_name().to_string();
                let (is_active, shadowed_by) = match active_by_key.get(&desc.key) {
                    Some(active_source) => (false, Some(active_source.clone())),
                    None => {
                        active_by_key.insert(desc.key.clone(), named.identity.clone());
                        (true, None)
                    }
                };
                entries.push(SkillIntrospectionEntry {
                    descriptor: desc,
                    source_identity: Some(named.identity.clone()),
                    shadowed_by: shadowed_by
                        .as_ref()
                        .map(|identity| identity.display_name.clone()),
                    shadowed_by_identity: shadowed_by.clone(),
                    shadowed_by_source_uuid: shadowed_by.map(|identity| identity.source_uuid),
                    is_active,
                });
            }
        }

        Ok(entries)
    }

    async fn load_from_source(
        &self,
        key: &SkillKey,
        source_name: Option<&str>,
    ) -> Result<SkillDocument, SkillError> {
        let canonical_key = self.resolve_key_for_load(key)?;
        let Some(target) = source_name else {
            return self.load_canonical(&canonical_key).await;
        };
        let target_uuid = {
            let parsed = SourceUuid::parse(target)?;
            let target_key = SkillKey::new(parsed, canonical_key.skill_name.clone());
            self.resolve_key_for_load(&target_key)?.source_uuid
        };
        for named in &self.sources {
            if named.source_uuid() == &target_uuid {
                let mut doc = named.source.load(&canonical_key).await?;
                doc.descriptor.source_name = named.display_name().to_string();
                return Ok(doc);
            }
        }
        Err(SkillError::NotFound { key: canonical_key })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::source::InMemorySkillSource;
    use indexmap::IndexMap;
    use meerkat_core::skills::{
        SkillKeyRemap, SkillName, SkillScope, SourceIdentityLineage, SourceIdentityLineageEvent,
    };

    fn source_uuid(raw: &str) -> SourceUuid {
        SourceUuid::parse(raw).unwrap()
    }

    fn skill_key(source_uuid: &SourceUuid, skill: &str) -> SkillKey {
        SkillKey::new(source_uuid.clone(), SkillName::parse(skill).unwrap())
    }

    fn skill_doc(key: SkillKey, display: &str, body: &str) -> SkillDocument {
        SkillDocument {
            descriptor: SkillDescriptor {
                key,
                name: display.to_string(),
                description: format!("description for {display}"),
                scope: SkillScope::Project,
                metadata: IndexMap::new(),
                capability_requirements: Vec::new(),
                source_name: String::new(),
            },
            body: body.to_string(),
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

    #[tokio::test]
    async fn load_applies_registry_remap_before_first_wins_lookup() {
        let legacy_source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let canonical_source = source_uuid("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb");
        let legacy_key = skill_key(&legacy_source, "alpha");
        let canonical_key = skill_key(&canonical_source, "alpha");
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
                    from: legacy_source.clone(),
                    to: canonical_source.clone(),
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
        let legacy = NamedSource::new(
            source_record(legacy_source, "legacy"),
            SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                legacy_key.clone(),
                "Legacy Alpha",
                "legacy body",
            )])),
        );
        let canonical = NamedSource::new(
            source_record(canonical_source, "canonical"),
            SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                canonical_key.clone(),
                "Canonical Alpha",
                "canonical body",
            )])),
        );
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![legacy, canonical],
            Arc::new(registry),
        );

        let doc = composite.load(&legacy_key).await.unwrap();

        assert_eq!(doc.descriptor.key, canonical_key);
        assert_eq!(doc.descriptor.name, "Canonical Alpha");
        assert_eq!(doc.descriptor.source_name, "canonical");
        assert_eq!(doc.body, "canonical body");
    }
}
