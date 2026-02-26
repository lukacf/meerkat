//! Default skill engine implementation.

use std::collections::HashSet;
use std::future::Future;

use meerkat_core::skills::{
    ResolvedSkill, SkillArtifact, SkillArtifactContent, SkillCollection, SkillDescriptor,
    SkillDocument, SkillEngine, SkillError, SkillFilter, SkillId, SkillIntrospectionEntry,
    SkillQuarantineDiagnostic, SkillSource, SourceHealthSnapshot,
};

use crate::renderer;

/// Default implementation of [`SkillEngine`].
pub struct DefaultSkillEngine<S>
where
    S: SkillSource + Send + Sync,
{
    source: S,
    available_capabilities: HashSet<String>,
    inventory_threshold: usize,
    max_injection_bytes: usize,
}

impl<S> DefaultSkillEngine<S>
where
    S: SkillSource + Send + Sync,
{
    pub fn new(source: S, available_capabilities: Vec<String>) -> Self {
        Self {
            source,
            available_capabilities: available_capabilities.into_iter().collect(),
            inventory_threshold: renderer::DEFAULT_INVENTORY_THRESHOLD,
            max_injection_bytes: renderer::MAX_INJECTION_BYTES,
        }
    }

    /// Set the inventory threshold from config (overrides default).
    pub fn with_inventory_threshold(mut self, threshold: usize) -> Self {
        self.inventory_threshold = threshold;
        self
    }

    /// Set the max injection bytes from config (overrides default).
    pub fn with_max_injection_bytes(mut self, max_bytes: usize) -> Self {
        self.max_injection_bytes = max_bytes;
        self
    }
}

/// Filter skills to those whose required capabilities are all available.
fn filter_by_capabilities(
    skills: Vec<SkillDescriptor>,
    available: &HashSet<String>,
) -> Vec<SkillDescriptor> {
    skills
        .into_iter()
        .filter(|s| {
            s.requires_capabilities
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
        ids: &[SkillId],
    ) -> impl Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send {
        async move {
            let mut results = Vec::new();
            for id in ids {
                let doc = self.source.load(id).await?;

                let missing: Vec<_> = doc
                    .descriptor
                    .requires_capabilities
                    .iter()
                    .filter(|cap| !self.available_capabilities.contains(*cap))
                    .collect();
                if !missing.is_empty() {
                    return Err(SkillError::Load(
                        format!(
                            "skill '{}' requires unavailable capabilities: {:?}",
                            id.0, missing
                        )
                        .into(),
                    ));
                }

                let rendered = renderer::render_injection_with_limit(
                    &id.0,
                    &doc.body,
                    self.max_injection_bytes,
                );
                let byte_size = rendered.len();
                results.push(ResolvedSkill {
                    id: id.clone(),
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
        id: &SkillId,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        async move { self.source.list_artifacts(id).await }
    }

    fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move { self.source.read_artifact(id, artifact_path).await }
    }

    fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        async move {
            self.source
                .invoke_function(id, function_name, arguments)
                .await
        }
    }

    fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillIntrospectionEntry>, SkillError>> + Send {
        async move {
            let entries = self.source.list_all_with_provenance(filter).await?;
            // Apply capability filtering: only active entries with available capabilities
            Ok(entries
                .into_iter()
                .filter(|e| {
                    // Shadowed entries pass through without capability filtering
                    // (they're inactive anyway).
                    !e.is_active
                        || e.descriptor
                            .requires_capabilities
                            .iter()
                            .all(|cap| self.available_capabilities.contains(cap))
                })
                .collect())
        }
    }

    fn load_from_source(
        &self,
        id: &SkillId,
        source_name: Option<&str>,
    ) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        let source_name = source_name.map(ToString::to_string);
        async move {
            self.source
                .load_from_source(id, source_name.as_deref())
                .await
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::source::InMemorySkillSource;
    use indexmap::IndexMap;
    use meerkat_core::skills::{SkillDescriptor, SkillDocument, SkillId, SkillScope};

    fn test_skill(id: &str, name: &str, caps: &[&str]) -> SkillDocument {
        SkillDocument {
            descriptor: SkillDescriptor {
                id: SkillId(id.to_string()),
                name: name.to_string(),
                description: format!("Description for {name}"),
                scope: SkillScope::Builtin,
                requires_capabilities: caps.iter().map(std::string::ToString::to_string).collect(),
                ..Default::default()
            },
            body: format!("# {name}\n\nContent for {id}."),
            extensions: IndexMap::new(),
        }
    }

    fn multi_source() -> InMemorySkillSource {
        InMemorySkillSource::new(vec![
            test_skill("extraction/email", "Email Extractor", &[]),
            test_skill("extraction/fiction", "Fiction Extractor", &[]),
            test_skill("formatting/markdown", "Markdown Output", &[]),
            test_skill("pdf-processing", "PDF Processing", &[]),
        ])
    }

    #[tokio::test]
    async fn test_inventory_section_uses_xml() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        let engine =
            DefaultSkillEngine::new(source, vec!["builtins".to_string(), "shell".to_string()]);

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("<available_skills>"));
        assert!(section.contains("<skill id=\"task-workflow\">"));
        assert!(section.contains("<skill id=\"shell-patterns\">"));
    }

    #[tokio::test]
    async fn test_inventory_filters_by_capabilities() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        let engine = DefaultSkillEngine::new(source, vec!["builtins".to_string()]);

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("<skill id=\"task-workflow\">"));
        assert!(!section.contains("shell-patterns"));
    }

    #[tokio::test]
    async fn test_resolve_and_render_returns_vec() {
        let source =
            InMemorySkillSource::new(vec![test_skill("task-workflow", "Task Workflow", &[])]);

        let engine = DefaultSkillEngine::new(source, vec![]);

        let result = engine
            .resolve_and_render(&[SkillId("task-workflow".into())])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert!(
            result[0]
                .rendered_body
                .contains("<skill id=\"task-workflow\">")
        );
        assert!(
            result[0]
                .rendered_body
                .contains("Content for task-workflow")
        );
        assert_eq!(result[0].name, "Task Workflow");
        assert!(result[0].byte_size > 0);
    }

    #[tokio::test]
    async fn test_resolve_and_render_unknown_id() {
        let source = InMemorySkillSource::new(vec![]);
        let engine = DefaultSkillEngine::new(source, vec![]);

        let result = engine
            .resolve_and_render(&[SkillId("nonexistent/skill".into())])
            .await;
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_preload_missing_skill_errors() {
        let source = InMemorySkillSource::new(vec![test_skill("existing/skill", "Existing", &[])]);
        let engine = DefaultSkillEngine::new(source, vec![]);

        let result = engine
            .resolve_and_render(&[
                SkillId("existing/skill".into()),
                SkillId("missing/skill".into()),
            ])
            .await;
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_list_skills_no_filter() {
        let engine = DefaultSkillEngine::new(multi_source(), vec![]);
        let skills = engine.list_skills(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 4);
    }

    #[tokio::test]
    async fn test_list_skills_collection_filter() {
        let engine = DefaultSkillEngine::new(multi_source(), vec![]);
        let skills = engine
            .list_skills(&SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(skills.len(), 2);
        assert!(skills.iter().all(|s| s.id.0.starts_with("extraction/")));
    }
}
