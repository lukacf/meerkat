//! Composite skill source that merges multiple named sources with precedence.

use crate::source::SourceNode;
use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillDescriptor, SkillDocument, SkillError, SkillFilter,
    SkillFunctionName, SkillFunctionOutput, SkillIntrospectionEntry, SkillKey,
    SkillQuarantineDiagnostic, SkillSource, SourceHealthSnapshot, SourceHealthState,
    SourceIdentityRecord, SourceIdentityRegistry, SourceUuid, apply_filter,
};
use std::collections::{HashMap, HashSet};
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
/// Listing and shadowing are computed over canonical keys from the attached
/// [`SourceIdentityRegistry`]: every raw key is resolved first, and a skill is
/// active iff it is the first source in precedence order that physically hosts
/// its canonical key — mirroring what `load` for that key actually serves.
/// Each skill's `source_name` field is populated from the `NamedSource.name`
/// that provided it.
pub struct CompositeSkillSource {
    sources: Vec<NamedSource>,
    registry: Option<Arc<SourceIdentityRegistry>>,
}

impl CompositeSkillSource {
    /// Create from named sources (precedence = order) with source-identity
    /// resolution applied before direct composite loads. This is the only
    /// constructor: a composite without an attached registry has no identity
    /// authority and every operation on it fails closed.
    pub fn from_named_with_registry(
        sources: Vec<NamedSource>,
        registry: Arc<SourceIdentityRegistry>,
    ) -> Self {
        Self {
            sources,
            registry: Some(registry),
        }
    }

    // Fail closed: composite loads and listings can only proceed once the
    // canonical source-identity registry has resolved the keys involved.
    // Without an attached registry there is no authority to confirm a key is
    // canonical / lifecycle-active, so we refuse rather than passing
    // unresolved keys straight through.
    fn require_registry(
        &self,
        action: std::fmt::Arguments<'_>,
    ) -> Result<&SourceIdentityRegistry, SkillError> {
        self.registry.as_deref().ok_or_else(|| {
            SkillError::Load(
                format!(
                    "composite skill source has no source-identity registry attached; \
                     refusing to {action} on unresolved identity"
                )
                .into(),
            )
        })
    }

    fn resolve_canonical(
        registry: &SourceIdentityRegistry,
        key: &SkillKey,
    ) -> Result<SkillKey, SkillError> {
        registry
            .resolve(key)
            .map(|resolved| resolved.key)
            .map_err(|e| {
                SkillError::Load(format!("source identity resolution failed for {key}: {e}").into())
            })
    }

    fn resolve_key_for_load(&self, key: &SkillKey) -> Result<SkillKey, SkillError> {
        let registry = self.require_registry(format_args!("load {key}"))?;
        Self::resolve_canonical(registry, key)
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
        let registry = self.require_registry(format_args!("list skills"))?;
        let mut merged: Vec<SkillDescriptor> = Vec::new();
        let mut seen: HashSet<SkillKey> = HashSet::new();
        for named in &self.sources {
            let items = named.source.list(filter).await?;
            for mut desc in items {
                let canonical = Self::resolve_canonical(registry, &desc.key)?;
                // Only the first source physically hosting the canonical key
                // is listed — the same copy `load` for that key serves.
                if desc.key != canonical || !seen.insert(canonical) {
                    continue;
                }
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
        function_name: &SkillFunctionName,
        arguments: meerkat_core::ToolCallArguments,
    ) -> Result<SkillFunctionOutput, SkillError> {
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
        let registry = self.require_registry(format_args!("list skills"))?;
        let mut collected: Vec<(SourceIdentityRecord, SkillDescriptor, SkillKey)> = Vec::new();
        let mut active_hosts: HashMap<SkillKey, (usize, SourceIdentityRecord)> = HashMap::new();

        for named in &self.sources {
            let items = named.source.list(filter).await?;
            for mut desc in items {
                desc.source_name = named.display_name().to_string();
                let canonical = Self::resolve_canonical(registry, &desc.key)?;
                // Key-exact sources can only serve keys they physically list,
                // so the active entry per canonical key is the first source in
                // precedence order whose raw key IS the canonical key —
                // exactly what `load` for that key serves. Remapped-away
                // copies are never active, regardless of order.
                if desc.key == canonical {
                    active_hosts
                        .entry(canonical.clone())
                        .or_insert_with(|| (collected.len(), named.identity.clone()));
                }
                collected.push((named.identity.clone(), desc, canonical));
            }
        }

        let entries = collected
            .into_iter()
            .enumerate()
            .map(|(index, (source_identity, mut descriptor, canonical))| {
                let (is_active, shadowed_by) = match active_hosts.get(&canonical) {
                    Some((active_index, _)) if *active_index == index => (true, None),
                    Some((_, host_identity)) => (false, Some(host_identity.clone())),
                    None => (false, None),
                };
                descriptor.key = canonical;
                SkillIntrospectionEntry {
                    descriptor,
                    source_identity: Some(source_identity),
                    shadowed_by: shadowed_by
                        .as_ref()
                        .map(|identity| identity.display_name.clone()),
                    shadowed_by_identity: shadowed_by.clone(),
                    shadowed_by_source_uuid: shadowed_by.map(|identity| identity.source_uuid),
                    is_active,
                }
            })
            .collect();

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
        SourceIdentityStatus, SourceTransportKind,
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

    fn remap_registry(
        legacy_source: &SourceUuid,
        canonical_source: &SourceUuid,
        legacy_key: &SkillKey,
        canonical_key: &SkillKey,
    ) -> SourceIdentityRegistry {
        SourceIdentityRegistry::build(
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
        .unwrap()
    }

    fn legacy_named_source(legacy_source: &SourceUuid, legacy_key: &SkillKey) -> NamedSource {
        NamedSource::new(
            source_record(legacy_source.clone(), "legacy"),
            SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                legacy_key.clone(),
                "Legacy Alpha",
                "legacy body",
            )])),
        )
    }

    fn canonical_named_source(
        canonical_source: &SourceUuid,
        canonical_key: &SkillKey,
    ) -> NamedSource {
        NamedSource::new(
            source_record(canonical_source.clone(), "canonical"),
            SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                canonical_key.clone(),
                "Canonical Alpha",
                "canonical body",
            )])),
        )
    }

    #[tokio::test]
    async fn load_applies_registry_remap_before_first_wins_lookup() {
        let legacy_source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let canonical_source = source_uuid("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb");
        let legacy_key = skill_key(&legacy_source, "alpha");
        let canonical_key = skill_key(&canonical_source, "alpha");
        let registry = remap_registry(
            &legacy_source,
            &canonical_source,
            &legacy_key,
            &canonical_key,
        );
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![
                legacy_named_source(&legacy_source, &legacy_key),
                canonical_named_source(&canonical_source, &canonical_key),
            ],
            Arc::new(registry),
        );

        let doc = composite.load(&legacy_key).await.unwrap();

        assert_eq!(doc.descriptor.key, canonical_key);
        assert_eq!(doc.descriptor.name, "Canonical Alpha");
        assert_eq!(doc.descriptor.source_name, "canonical");
        assert_eq!(doc.body, "canonical body");
    }

    #[tokio::test]
    async fn list_all_with_provenance_canonicalizes_before_shadowing() {
        // The remapped-away legacy copy is listed BEFORE the canonical host:
        // the host copy must still be the single active entry, and it must be
        // the copy `load` for the canonical key actually serves.
        let legacy_source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let canonical_source = source_uuid("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb");
        let legacy_key = skill_key(&legacy_source, "alpha");
        let canonical_key = skill_key(&canonical_source, "alpha");
        let registry = remap_registry(
            &legacy_source,
            &canonical_source,
            &legacy_key,
            &canonical_key,
        );
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![
                legacy_named_source(&legacy_source, &legacy_key),
                canonical_named_source(&canonical_source, &canonical_key),
            ],
            Arc::new(registry),
        );

        let entries = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|entry| entry.descriptor.key == canonical_key),
            "all copies must carry the canonical key; got {entries:?}"
        );

        let active: Vec<_> = entries.iter().filter(|entry| entry.is_active).collect();
        assert_eq!(
            active.len(),
            1,
            "exactly one active entry per canonical key; got {entries:?}"
        );
        let active = active[0];
        let loaded = composite.load(&canonical_key).await.unwrap();
        assert_eq!(active.descriptor.name, loaded.descriptor.name);
        assert_eq!(active.descriptor.description, loaded.descriptor.description);
        assert_eq!(active.descriptor.source_name, loaded.descriptor.source_name);
        assert_eq!(active.descriptor.name, "Canonical Alpha");
        assert_eq!(
            active
                .source_identity
                .as_ref()
                .map(|identity| identity.source_uuid.clone()),
            Some(canonical_source.clone())
        );
        assert!(active.shadowed_by.is_none());

        let shadowed = entries.iter().find(|entry| !entry.is_active).unwrap();
        assert_eq!(shadowed.descriptor.name, "Legacy Alpha");
        assert_eq!(
            shadowed
                .source_identity
                .as_ref()
                .map(|identity| identity.source_uuid.clone()),
            Some(legacy_source.clone())
        );
        assert_eq!(shadowed.shadowed_by.as_deref(), Some("canonical"));
        assert_eq!(
            shadowed
                .shadowed_by_identity
                .as_ref()
                .map(|identity| identity.source_uuid.clone()),
            Some(canonical_source.clone())
        );
        assert_eq!(
            shadowed.shadowed_by_source_uuid,
            Some(canonical_source.clone())
        );
    }

    #[tokio::test]
    async fn list_all_with_provenance_same_source_rename_marks_new_key_active() {
        let source = source_uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
        let old_key = skill_key(&source, "old-name");
        let new_key = skill_key(&source, "new-name");
        let registry = SourceIdentityRegistry::build(
            vec![source_record(source.clone(), "project")],
            Vec::new(),
            vec![SkillKeyRemap {
                from: old_key.clone(),
                to: new_key.clone(),
                reason: Some("rename".to_string()),
            }],
            Vec::new(),
        )
        .unwrap();
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![NamedSource::new(
                source_record(source.clone(), "project"),
                SourceNode::Memory(InMemorySkillSource::new(vec![
                    skill_doc(old_key.clone(), "Old Name", "old body"),
                    skill_doc(new_key.clone(), "New Name", "new body"),
                ])),
            )],
            Arc::new(registry),
        );

        let entries = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.descriptor.key == new_key));
        let active: Vec<_> = entries.iter().filter(|entry| entry.is_active).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].descriptor.name, "New Name");
        let shadowed = entries.iter().find(|entry| !entry.is_active).unwrap();
        assert_eq!(shadowed.descriptor.name, "Old Name");
        assert_eq!(shadowed.shadowed_by.as_deref(), Some("project"));
        assert_eq!(shadowed.shadowed_by_source_uuid, Some(source.clone()));
    }

    #[tokio::test]
    async fn list_all_with_provenance_without_canonical_host_marks_all_copies_inactive() {
        // The remap target is listed by no source: no copy may claim active
        // (load for the canonical key returns NotFound), and there is no host
        // identity to blame, so shadowed_by stays None.
        let legacy_source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let canonical_source = source_uuid("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb");
        let legacy_key = skill_key(&legacy_source, "alpha");
        let canonical_key = skill_key(&canonical_source, "alpha");
        let registry = remap_registry(
            &legacy_source,
            &canonical_source,
            &legacy_key,
            &canonical_key,
        );
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![legacy_named_source(&legacy_source, &legacy_key)],
            Arc::new(registry),
        );

        let entries = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.descriptor.key, canonical_key);
        assert!(!entry.is_active);
        assert!(entry.shadowed_by.is_none());
        assert!(entry.shadowed_by_identity.is_none());
        assert!(entry.shadowed_by_source_uuid.is_none());

        let err = composite.load(&canonical_key).await.unwrap_err();
        assert!(matches!(err, SkillError::NotFound { .. }));
        let listed = composite.list(&SkillFilter::default()).await.unwrap();
        assert!(
            listed.is_empty(),
            "a canonical key without a host must not be listed; got {listed:?}"
        );
    }

    #[tokio::test]
    async fn list_all_with_provenance_first_host_wins_for_duplicate_keys() {
        let source = source_uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");
        let mirror = source_uuid("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");
        let key = skill_key(&source, "alpha");
        let registry = SourceIdentityRegistry::build(
            vec![
                source_record(source.clone(), "first"),
                source_record(mirror.clone(), "second"),
            ],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![
                NamedSource::new(
                    source_record(source.clone(), "first"),
                    SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                        key.clone(),
                        "First Alpha",
                        "first body",
                    )])),
                ),
                NamedSource::new(
                    source_record(mirror.clone(), "second"),
                    SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                        key.clone(),
                        "Second Alpha",
                        "second body",
                    )])),
                ),
            ],
            Arc::new(registry),
        );

        let entries = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        let active: Vec<_> = entries.iter().filter(|entry| entry.is_active).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].descriptor.name, "First Alpha");
        let shadowed = entries.iter().find(|entry| !entry.is_active).unwrap();
        assert_eq!(shadowed.descriptor.name, "Second Alpha");
        assert_eq!(shadowed.shadowed_by.as_deref(), Some("first"));
        assert_eq!(shadowed.shadowed_by_source_uuid, Some(source.clone()));
    }

    #[tokio::test]
    async fn list_returns_exactly_one_descriptor_per_canonical_key() {
        let legacy_source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let canonical_source = source_uuid("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb");
        let legacy_key = skill_key(&legacy_source, "alpha");
        let canonical_key = skill_key(&canonical_source, "alpha");
        let registry = remap_registry(
            &legacy_source,
            &canonical_source,
            &legacy_key,
            &canonical_key,
        );
        let composite = CompositeSkillSource::from_named_with_registry(
            vec![
                legacy_named_source(&legacy_source, &legacy_key),
                canonical_named_source(&canonical_source, &canonical_key),
            ],
            Arc::new(registry),
        );

        let listed = composite.list(&SkillFilter::default()).await.unwrap();

        assert_eq!(listed.len(), 1, "one canonical skill, one descriptor");
        assert_eq!(listed[0].key, canonical_key);
        assert_eq!(listed[0].name, "Canonical Alpha");
        assert_eq!(listed[0].source_name, "canonical");
    }

    #[tokio::test]
    async fn load_without_registry_fails_closed() {
        // Row #11: a composite with no attached source-identity registry must
        // refuse to load or list (no silent raw-key pass-through anywhere).
        // No public constructor can build this state — the negative case is
        // assembled through the private fields to pin the rejection machinery.
        let source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let key = skill_key(&source, "alpha");
        let composite = CompositeSkillSource {
            sources: vec![NamedSource::new(
                source_record(source, "legacy"),
                SourceNode::Memory(InMemorySkillSource::new(vec![skill_doc(
                    key.clone(),
                    "Alpha",
                    "body",
                )])),
            )],
            registry: None,
        };

        let err = composite.load(&key).await.unwrap_err();
        assert!(
            matches!(&err, SkillError::Load(message) if message.contains("no source-identity registry")),
            "expected fail-closed registry-absence refusal; got {err:?}"
        );

        let err = composite.list(&SkillFilter::default()).await.unwrap_err();
        assert!(
            matches!(&err, SkillError::Load(message) if message.contains("no source-identity registry")),
            "list must fail closed without a registry; got {err:?}"
        );

        let err = composite
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, SkillError::Load(message) if message.contains("no source-identity registry")),
            "provenance listing must fail closed without a registry; got {err:?}"
        );
    }
}
