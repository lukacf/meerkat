//! Default skill engine implementation.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use meerkat_core::skills::{
    CapabilityId, ResolvedSkill, SkillArtifact, SkillArtifactContent, SkillCollection,
    SkillDescriptor, SkillDocument, SkillEngine, SkillError, SkillFilter, SkillIntrospectionEntry,
    SkillKey, SkillQuarantineDiagnostic, SkillRef, SkillSource, SourceHealthSnapshot,
    SourceIdentityRegistry,
};

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
    /// Optional source-identity registry used to apply lineage remap
    /// chains on incoming `SkillKey`s. When absent (tests, minimal
    /// embeddings), `canonical_skill_key` is the identity function; the
    /// pre-V4 behavior is preserved. When present (facade-constructed
    /// engines), the builtin skill tools get lineage-aware key
    /// resolution.
    registry: Option<Arc<SourceIdentityRegistry>>,
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
            registry: None,
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

    /// Attach a `SourceIdentityRegistry` so `canonical_skill_key`
    /// applies the lineage remap chain. Without one, the engine falls
    /// back to the identity projection.
    pub fn with_source_identity_registry(mut self, registry: Arc<SourceIdentityRegistry>) -> Self {
        self.registry = Some(registry);
        self
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
            let all_skills = self.source.list(&SkillFilter::default()).await?;
            let available = filter_by_capabilities(all_skills, &self.available_capabilities);
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
                let doc = self.source.load(key).await?;

                if let Some(missing) = doc
                    .descriptor
                    .capability_requirements
                    .iter()
                    .find(|cap| !self.available_capabilities.contains(*cap))
                {
                    return Err(SkillError::CapabilityUnavailable {
                        key: key.clone(),
                        capability: missing.clone(),
                    });
                }

                let rendered =
                    renderer::render_injection_with_limit(key, &doc.body, self.max_injection_bytes);
                let byte_size = rendered.len();
                results.push(ResolvedSkill {
                    key: key.clone(),
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
            Ok(filter_by_capabilities(
                all_skills,
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
        async move { self.source.list_artifacts(key).await }
    }

    fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move { self.source.read_artifact(key, artifact_path).await }
    }

    fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        async move {
            self.source
                .invoke_function(key, function_name, arguments)
                .await
        }
    }

    fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillIntrospectionEntry>, SkillError>> + Send {
        async move {
            let entries = self.source.list_all_with_provenance(filter).await?;
            Ok(entries
                .into_iter()
                .filter(|e| {
                    !e.is_active
                        || e.descriptor
                            .capability_requirements
                            .iter()
                            .all(|cap| self.available_capabilities.contains(cap))
                })
                .collect())
        }
    }

    fn load_from_source(
        &self,
        key: &SkillKey,
        source_name: Option<&str>,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        let source_name = source_name.map(ToString::to_string);
        async move {
            self.source
                .load_from_source(key, source_name.as_deref())
                .await
        }
    }

    fn canonical_skill_key(
        &self,
        key: &SkillKey,
    ) -> impl Future<Output = Result<SkillKey, SkillError>> + Send {
        let registry = self.registry.clone();
        let reference = SkillRef::from(key.clone());
        async move {
            match registry {
                Some(reg) => reg.canonical_skill_key(&reference),
                None => Ok(reference.into_key()),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::source::InMemorySkillSource;
    use indexmap::IndexMap;
    use meerkat_core::skills::{SkillName, SkillScope, SourceUuid};

    fn test_key(skill: &str) -> SkillKey {
        SkillKey {
            source_uuid: SourceUuid::builtin(),
            skill_name: SkillName::parse(skill).unwrap(),
        }
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
