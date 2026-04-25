//! Composite skill source that merges multiple named sources with precedence.

use crate::source::SourceNode;
use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillDescriptor, SkillDocument, SkillError, SkillFilter,
    SkillIntrospectionEntry, SkillKey, SkillQuarantineDiagnostic, SkillSource,
    SourceHealthSnapshot, SourceHealthState, SourceUuid, apply_filter,
};
use std::collections::HashMap;

/// A named skill source for provenance tracking.
pub struct NamedSource {
    pub name: String,
    pub source_uuid: SourceUuid,
    pub source: SourceNode,
}

/// Merges skills from multiple named sources with precedence.
///
/// First source wins for duplicate `SkillKey`s. Each skill's `source_name`
/// field is populated from the `NamedSource.name` that provided it.
pub struct CompositeSkillSource {
    sources: Vec<NamedSource>,
}

impl CompositeSkillSource {
    /// Create from named sources (precedence = order).
    pub fn from_named(sources: Vec<NamedSource>) -> Self {
        Self { sources }
    }

    /// Create from unnamed sources (backward compatibility).
    pub fn new(sources: Vec<SourceNode>) -> Self {
        let named = sources
            .into_iter()
            .enumerate()
            .map(|(i, source)| NamedSource {
                name: format!("source_{i}"),
                source_uuid: SourceUuid::builtin(),
                source,
            })
            .collect();
        Self { sources: named }
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
                desc.source_name = named.name.clone();
                merged.push(desc);
            }
        }
        Ok(apply_filter(&merged, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        for named in &self.sources {
            match named.source.load(key).await {
                Ok(mut doc) => {
                    doc.descriptor.source_name = named.name.clone();
                    return Ok(doc);
                }
                Err(SkillError::NotFound { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(SkillError::NotFound { key: key.clone() })
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
        let mut active_by_key: HashMap<SkillKey, (String, SourceUuid)> = HashMap::new();

        for named in &self.sources {
            let items = named.source.list(filter).await?;
            for mut desc in items {
                desc.source_name = named.name.clone();
                let (is_active, shadowed_by) = match active_by_key.get(&desc.key) {
                    Some((active_source, active_uuid)) => {
                        (false, Some((active_source.clone(), active_uuid.clone())))
                    }
                    None => {
                        active_by_key.insert(
                            desc.key.clone(),
                            (named.name.clone(), named.source_uuid.clone()),
                        );
                        (true, None)
                    }
                };
                entries.push(SkillIntrospectionEntry {
                    descriptor: desc,
                    shadowed_by: shadowed_by.as_ref().map(|(name, _)| name.clone()),
                    shadowed_by_source_uuid: shadowed_by.map(|(_, source_uuid)| source_uuid),
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
        let Some(target) = source_name else {
            return self.load(key).await;
        };
        let target_uuid = SourceUuid::parse(target)?;
        for named in &self.sources {
            if named.source_uuid == target_uuid {
                let mut doc = named.source.load(key).await?;
                doc.descriptor.source_name = named.name.clone();
                return Ok(doc);
            }
        }
        Err(SkillError::NotFound { key: key.clone() })
    }
}
