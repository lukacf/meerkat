//! Default skill engine implementation.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use meerkat_core::skills::{
    CapabilityId, ResolvedSkill, SkillArtifact, SkillArtifactContent, SkillCollection,
    SkillDescriptor, SkillDocument, SkillEngine, SkillError, SkillFilter, SkillIntrospectionEntry,
    SkillKey, SkillQuarantineDiagnostic, SkillRef, SkillSource, SourceHealthSnapshot,
    SourceIdentityRecord, SourceIdentityRegistry, SourceUuid,
};
use meerkat_core::skills_config::default_source_identity_records;

use crate::renderer;

/// Default implementation of [`SkillEngine`].
pub struct DefaultSkillEngine<S>
where
    S: SkillSource + Send + Sync,
{
    source: S,
    available_capabilities: HashSet<CapabilityId>,
    inventory_threshold: usize,
    max_injection_bytes: usize,
    /// Source-identity registry used to apply lineage remap chains and source
    /// lifecycle gates on incoming `SkillKey`s.
    registry: Arc<SourceIdentityRegistry>,
}

impl<S> DefaultSkillEngine<S>
where
    S: SkillSource + Send + Sync,
{
    pub fn new(source: S, available_capabilities: Vec<CapabilityId>) -> Self {
        Self {
            source,
            available_capabilities: available_capabilities.into_iter().collect(),
            inventory_threshold: renderer::DEFAULT_INVENTORY_THRESHOLD,
            max_injection_bytes: renderer::MAX_INJECTION_BYTES,
            registry: Arc::new(default_source_identity_registry()),
        }
    }

    pub fn with_inventory_threshold(mut self, threshold: usize) -> Self {
        self.inventory_threshold = threshold;
        self
    }

    pub fn with_max_injection_bytes(mut self, max_bytes: usize) -> Self {
        self.max_injection_bytes = max_bytes;
        self
    }

    pub fn with_source_identity_registry(mut self, registry: Arc<SourceIdentityRegistry>) -> Self {
        self.registry = registry;
        self
    }

    fn resolve_key(&self, key: &SkillKey) -> Result<SkillKey, SkillError> {
        self.registry
            .resolve(key)
            .map(|resolved| resolved.key)
            .map_err(|e| {
                SkillError::Load(format!("source identity resolution failed for {key}: {e}").into())
            })
    }

    fn resolve_source_identity(
        &self,
        key: &SkillKey,
    ) -> Result<(SkillKey, SourceIdentityRecord), SkillError> {
        self.registry
            .resolve(key)
            .map(|resolved| (resolved.key, resolved.source.clone()))
            .map_err(|e| {
                SkillError::Load(format!("source identity resolution failed for {key}: {e}").into())
            })
    }
}

fn default_source_identity_registry() -> SourceIdentityRegistry {
    match SourceIdentityRegistry::build(
        default_source_identity_records(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ) {
        Ok(registry) => registry,
        Err(err) => unreachable!("builtin/project source identity records are invalid: {err}"),
    }
}

fn filter_by_capabilities(
    skills: Vec<SkillDescriptor>,
    available: &HashSet<CapabilityId>,
) -> Vec<SkillDescriptor> {
    skills
        .into_iter()
        .filter(|s| {
            s.capability_requirements
                .iter()
                .all(|cap| available.contains(cap))
        })
        .collect()
}

#[allow(clippy::manual_async_fn)]
impl<S> SkillEngine for DefaultSkillEngine<S>
where
    S: SkillSource + Send + Sync,
{
    fn inventory_section(&self) -> impl Future<Output = Result<String, SkillError>> + Send {
        async move {
            let entries = self
                .list_all_with_provenance(&SkillFilter::default())
                .await?;
            let available: Vec<_> = entries
                .into_iter()
                .filter(|entry| entry.is_active)
                .map(|entry| entry.descriptor)
                .collect();
            let collections = meerkat_core::skills::derive_collections(&available);
            Ok(renderer::render_inventory(
                &available,
                &collections,
                self.inventory_threshold,
            ))
        }
    }

    fn resolve_and_render(
        &self,
        keys: &[SkillKey],
    ) -> impl Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send {
        async move {
            let mut results = Vec::new();
            for key in keys {
                let canonical_key = self.resolve_key(key)?;
                let doc = self.source.load(&canonical_key).await?;

                if let Some(missing) = doc
                    .descriptor
                    .capability_requirements
                    .iter()
                    .find(|cap| !self.available_capabilities.contains(*cap))
                {
                    return Err(SkillError::CapabilityUnavailable {
                        key: canonical_key.clone(),
                        capability: missing.clone(),
                    });
                }

                let rendered = renderer::render_injection_with_limit(
                    &canonical_key,
                    &doc.body,
                    self.max_injection_bytes,
                );
                let byte_size = rendered.len();
                results.push(ResolvedSkill {
                    key: canonical_key,
                    name: doc.descriptor.name.clone(),
                    rendered_body: rendered,
                    byte_size,
                });
            }
            Ok(results)
        }
    }

    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
        async move { self.source.collections().await }
    }

    fn list_skills(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
        async move {
            let all_skills = self.source.list(filter).await?;
            let active_skills: Vec<_> = all_skills
                .into_iter()
                .filter(|descriptor| self.resolve_key(&descriptor.key).is_ok())
                .collect();
            Ok(filter_by_capabilities(
                active_skills,
                &self.available_capabilities,
            ))
        }
    }

    fn quarantined_diagnostics(
        &self,
    ) -> impl Future<Output = Result<Vec<SkillQuarantineDiagnostic>, SkillError>> + Send {
        async move { self.source.quarantined_diagnostics().await }
    }

    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<SourceHealthSnapshot, SkillError>> + Send {
        async move { self.source.health_snapshot().await }
    }

    fn list_artifacts(
        &self,
        key: &SkillKey,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        async move {
            let canonical_key = self.resolve_key(key)?;
            self.source.list_artifacts(&canonical_key).await
        }
    }

    fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move {
            let canonical_key = self.resolve_key(key)?;
            self.source
                .read_artifact(&canonical_key, artifact_path)
                .await
        }
    }

    fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &meerkat_core::skills::SkillFunctionName,
        arguments: meerkat_core::ToolCallArguments,
    ) -> impl Future<Output = Result<meerkat_core::skills::SkillFunctionResult, SkillError>> + Send
    {
        async move {
            let canonical_key = self.resolve_key(key)?;
            self.source
                .invoke_function(&canonical_key, function_name, arguments)
                .await
        }
    }

    fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillIntrospectionEntry>, SkillError>> + Send {
        async move {
            let entries = self.source.list_all_with_provenance(filter).await?;
            let mut active_entries = Vec::new();
            for mut entry in entries {
                let (canonical_key, source_identity) =
                    self.resolve_source_identity(&entry.descriptor.key)?;
                if entry.is_active
                    && !entry
                        .descriptor
                        .capability_requirements
                        .iter()
                        .all(|cap| self.available_capabilities.contains(cap))
                {
                    continue;
                }
                let skill_name = canonical_key.skill_name.clone();
                entry.descriptor.key = canonical_key;
                entry.descriptor.source_name = source_identity.display_name.clone();
                entry.source_identity = Some(source_identity);
                if let Some(source_uuid) = entry.shadowed_by_source_uuid.clone() {
                    let shadow_key = SkillKey::new(source_uuid, skill_name);
                    let (_, shadow_identity) = self.resolve_source_identity(&shadow_key)?;
                    entry.shadowed_by = Some(shadow_identity.display_name.clone());
                    entry.shadowed_by_identity = Some(shadow_identity);
                }
                active_entries.push(entry);
            }
            Ok(active_entries)
        }
    }

    fn load_from_source(
        &self,
        key: &SkillKey,
        source_uuid: Option<&SourceUuid>,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        let source_uuid = source_uuid.cloned();
        async move {
            let canonical_key = self.resolve_key(key)?;
            self.source
                .load_from_source(&canonical_key, source_uuid.as_ref())
                .await
        }
    }

    fn canonical_skill_key(
        &self,
        key: &SkillKey,
    ) -> impl Future<Output = Result<SkillKey, SkillError>> + Send {
        let registry = self.registry.clone();
        let reference = SkillRef::from(key.clone());
        async move { registry.canonical_skill_key(&reference) }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::source::{CompositeSkillSource, InMemorySkillSource, NamedSource, SourceNode};
    use indexmap::IndexMap;
    use meerkat_core::skills::{
        SkillName, SkillScope, SourceIdentityRecord, SourceIdentityStatus, SourceTransportKind,
        SourceUuid,
    };

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::builtin(),
            skill_name: SkillName::parse(skill).unwrap(),
        }
    }

    fn source_uuid(raw: &str) -> SourceUuid {
        SourceUuid::parse(raw).unwrap()
    }

    fn source_record(
        source_uuid: SourceUuid,
        display_name: &str,
        status: SourceIdentityStatus,
    ) -> SourceIdentityRecord {
        SourceIdentityRecord {
            source_uuid,
            display_name: display_name.to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: format!("fixture:{display_name}"),
            status,
        }
    }

    fn test_skill_from_source(source_uuid: SourceUuid, skill: &str, name: &str) -> SkillDocument {
        let mut doc = test_skill(skill, name, &[]);
        doc.descriptor.key.source_uuid = source_uuid;
        doc
    }

    fn test_skill(skill: &str, name: &str, caps: &[&str]) -> SkillDocument {
        SkillDocument {
            descriptor: SkillDescriptor {
                key: test_key(skill),
                name: name.to_string(),
                description: format!("Description for {name}"),
                scope: SkillScope::Builtin,
                metadata: IndexMap::new(),
                capability_requirements: caps
                    .iter()
                    .map(|c| CapabilityId::parse(c).unwrap())
                    .collect(),
                source_name: String::new(),
            },
            body: format!("# {name}\n\nContent for {skill}."),
            extensions: IndexMap::new(),
        }
    }

    #[tokio::test]
    async fn test_inventory_section_uses_xml() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        let engine = DefaultSkillEngine::new(
            source,
            vec![
                CapabilityId::parse("builtins").unwrap(),
                CapabilityId::parse("shell").unwrap(),
            ],
        );

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("<available_skills>"));
        assert!(section.contains("task-workflow"));
        assert!(section.contains("shell-patterns"));
    }

    #[tokio::test]
    async fn test_inventory_filters_by_capabilities() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        let engine =
            DefaultSkillEngine::new(source, vec![CapabilityId::parse("builtins").unwrap()]);

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("task-workflow"));
        assert!(!section.contains("shell-patterns"));
    }

    #[tokio::test]
    async fn inventory_section_reports_source_identity_resolution_failures() {
        let unknown_source = source_uuid("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa");
        let source = InMemorySkillSource::new(vec![test_skill_from_source(
            unknown_source.clone(),
            "orphaned",
            "Orphaned",
        )]);
        let registry = SourceIdentityRegistry::default();
        let engine = DefaultSkillEngine::new(source, vec![])
            .with_source_identity_registry(Arc::new(registry));

        let err = engine.inventory_section().await.unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains("source identity resolution failed"),
            "inventory error must name source identity resolution; got {message}"
        );
        assert!(
            message.contains(&unknown_source.to_string()),
            "inventory error must identify the failing source; got {message}"
        );
        assert!(
            message.contains("source unknown"),
            "inventory error must include the registry failure; got {message}"
        );
    }

    #[tokio::test]
    async fn list_all_with_provenance_reports_source_identity_failures() {
        let disabled_source = source_uuid("bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb");
        let source = InMemorySkillSource::new(vec![test_skill_from_source(
            disabled_source.clone(),
            "disabled",
            "Disabled",
        )]);
        let registry = SourceIdentityRegistry::build(
            vec![source_record(
                disabled_source.clone(),
                "disabled fixture",
                SourceIdentityStatus::Disabled,
            )],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let engine = DefaultSkillEngine::new(source, vec![])
            .with_source_identity_registry(Arc::new(registry));

        let err = engine
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains("source identity resolution failed"),
            "provenance listing error must name source identity resolution; got {message}"
        );
        assert!(
            message.contains(&disabled_source.to_string()),
            "provenance listing error must identify the failing source; got {message}"
        );
        assert!(
            message.contains("Disabled"),
            "provenance listing error must include the registry status; got {message}"
        );
    }

    #[tokio::test]
    async fn list_skills_preserves_valid_entries_when_another_source_identity_fails() {
        let active_source = source_uuid("99999999-9999-4999-8999-999999999999");
        let unknown_source = source_uuid("88888888-8888-4888-8888-888888888888");
        let source = InMemorySkillSource::new(vec![
            test_skill_from_source(active_source.clone(), "valid", "Valid"),
            test_skill_from_source(unknown_source, "orphaned", "Orphaned"),
        ]);
        let registry = SourceIdentityRegistry::build(
            vec![source_record(
                active_source.clone(),
                "active fixture",
                SourceIdentityStatus::Active,
            )],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let engine = DefaultSkillEngine::new(source, vec![])
            .with_source_identity_registry(Arc::new(registry));

        let listed = engine.list_skills(&SkillFilter::default()).await.unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key.source_uuid, active_source);
        assert_eq!(listed[0].key.skill_name.as_str(), "valid");
    }

    #[tokio::test]
    async fn list_all_with_provenance_reports_composite_source_identity_failures() {
        let active_source = source_uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
        let unknown_source = source_uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");
        let skill = "shadowed";
        let unknown_key = SkillKey::new(unknown_source.clone(), SkillName::parse(skill).unwrap());
        let active_named = NamedSource::new(
            source_record(
                active_source.clone(),
                "active fixture",
                SourceIdentityStatus::Active,
            ),
            SourceNode::Memory(InMemorySkillSource::new(vec![test_skill_from_source(
                active_source.clone(),
                skill,
                "Active",
            )])),
        );
        let unknown_named = NamedSource::new(
            source_record(
                unknown_source.clone(),
                "unknown fixture",
                SourceIdentityStatus::Active,
            ),
            SourceNode::Memory(InMemorySkillSource::new(vec![test_skill_from_source(
                unknown_source,
                skill,
                "Unknown",
            )])),
        );
        let registry = SourceIdentityRegistry::build(
            vec![source_record(
                active_source,
                "active fixture",
                SourceIdentityStatus::Active,
            )],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let composite = CompositeSkillSource::from_named(vec![active_named, unknown_named]);
        let engine = DefaultSkillEngine::new(composite, vec![])
            .with_source_identity_registry(Arc::new(registry));

        let err = engine
            .list_all_with_provenance(&SkillFilter::default())
            .await
            .unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains(&unknown_key.to_string()),
            "composite provenance listing must identify the failing source; got {message}"
        );
        assert!(
            message.contains("source unknown"),
            "composite provenance listing must include the registry failure; got {message}"
        );
    }

    #[tokio::test]
    async fn direct_load_from_source_reports_source_identity_failures() {
        let disabled_source = source_uuid("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");
        let source = InMemorySkillSource::new(vec![test_skill_from_source(
            disabled_source.clone(),
            "disabled-load",
            "Disabled Load",
        )]);
        let registry = SourceIdentityRegistry::build(
            vec![source_record(
                disabled_source.clone(),
                "disabled fixture",
                SourceIdentityStatus::Disabled,
            )],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let engine = DefaultSkillEngine::new(source, vec![])
            .with_source_identity_registry(Arc::new(registry));
        let key = SkillKey::new(
            disabled_source.clone(),
            SkillName::parse("disabled-load").unwrap(),
        );

        let err = engine.load_from_source(&key, None).await.unwrap_err();
        let message = err.to_string();

        assert!(
            message.contains("source identity resolution failed"),
            "direct load error must name source identity resolution; got {message}"
        );
        assert!(
            message.contains(&disabled_source.to_string()),
            "direct load error must identify the failing source; got {message}"
        );
    }

    #[tokio::test]
    async fn test_resolve_and_render_returns_vec() {
        let source =
            InMemorySkillSource::new(vec![test_skill("task-workflow", "Task Workflow", &[])]);

        let engine = DefaultSkillEngine::new(source, vec![]);

        let result = engine
            .resolve_and_render(&[test_key("task-workflow")])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].rendered_body.contains("task-workflow"));
        assert!(
            result[0]
                .rendered_body
                .contains("Content for task-workflow")
        );
        assert_eq!(result[0].name, "Task Workflow");
        assert!(result[0].byte_size > 0);
    }

    #[tokio::test]
    async fn test_resolve_and_render_unknown_key() {
        let source = InMemorySkillSource::new(vec![]);
        let engine = DefaultSkillEngine::new(source, vec![]);

        let result = engine.resolve_and_render(&[test_key("nonexistent")]).await;
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_resolve_reports_unavailable_capability() {
        let source = InMemorySkillSource::new(vec![test_skill(
            "shell-patterns",
            "Shell Patterns",
            &["shell"],
        )]);
        let engine = DefaultSkillEngine::new(source, vec![]);

        let result = engine
            .resolve_and_render(&[test_key("shell-patterns")])
            .await;
        assert!(matches!(
            result,
            Err(SkillError::CapabilityUnavailable { .. })
        ));
    }
}
