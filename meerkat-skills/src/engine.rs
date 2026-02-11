//! Default skill engine implementation.

use std::collections::HashSet;

use async_trait::async_trait;
use meerkat_core::skills::{
    ResolvedSkill, SkillCollection, SkillDescriptor, SkillEngine, SkillError, SkillFilter, SkillId,
    SkillSource,
};

use crate::renderer;

/// Default implementation of [`SkillEngine`].
pub struct DefaultSkillEngine {
    source: Box<dyn SkillSource>,
    available_capabilities: HashSet<String>,
    inventory_threshold: usize,
    max_injection_bytes: usize,
}

impl DefaultSkillEngine {
    pub fn new(source: Box<dyn SkillSource>, available_capabilities: Vec<String>) -> Self {
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

#[async_trait]
impl SkillEngine for DefaultSkillEngine {
    async fn inventory_section(&self) -> Result<String, SkillError> {
        let all_skills = self.source.list(&SkillFilter::default()).await?;
        let available = filter_by_capabilities(all_skills, &self.available_capabilities);
        let collections = self.source.collections().await?;
        Ok(renderer::render_inventory(
            &available,
            &collections,
            self.inventory_threshold,
        ))
    }

    async fn resolve_and_render(
        &self,
        ids: &[SkillId],
    ) -> Result<Vec<ResolvedSkill>, SkillError> {
        let mut results = Vec::new();
        for id in ids {
            let doc = self.source.load(id).await?;

            // Enforce capability gating: reject skills whose required
            // capabilities are not available, even if the caller knows the ID.
            let missing: Vec<_> = doc.descriptor.requires_capabilities.iter()
                .filter(|cap| !self.available_capabilities.contains(*cap))
                .collect();
            if !missing.is_empty() {
                return Err(SkillError::Load(
                    format!("skill '{}' requires unavailable capabilities: {:?}", id.0, missing).into(),
                ));
            }

            let rendered = renderer::render_injection_with_limit(
                &id.0, &doc.body, self.max_injection_bytes,
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

    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        self.source.collections().await
    }

    async fn list_skills(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillDescriptor>, SkillError> {
        let all_skills = self.source.list(filter).await?;
        Ok(filter_by_capabilities(
            all_skills,
            &self.available_capabilities,
        ))
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
                requires_capabilities: caps.iter().map(|s| s.to_string()).collect(),
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

    // --- Inventory ---

    #[tokio::test]
    async fn test_inventory_section_uses_xml() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        let engine = DefaultSkillEngine::new(
            Box::new(source),
            vec!["builtins".to_string(), "shell".to_string()],
        );

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

        let engine =
            DefaultSkillEngine::new(Box::new(source), vec!["builtins".to_string()]);

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("<skill id=\"task-workflow\">"));
        assert!(!section.contains("shell-patterns"));
    }

    // --- resolve_and_render ---

    #[tokio::test]
    async fn test_resolve_and_render_returns_vec() {
        let source = InMemorySkillSource::new(vec![test_skill(
            "task-workflow",
            "Task Workflow",
            &[],
        )]);

        let engine = DefaultSkillEngine::new(Box::new(source), vec![]);

        let result = engine
            .resolve_and_render(&[SkillId("task-workflow".into())])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0]
            .rendered_body
            .contains("<skill id=\"task-workflow\">"));
        assert!(result[0].rendered_body.contains("Content for task-workflow"));
        assert_eq!(result[0].name, "Task Workflow");
        assert!(result[0].byte_size > 0);
    }

    #[tokio::test]
    async fn test_resolve_and_render_unknown_id() {
        let source = InMemorySkillSource::new(vec![]);
        let engine = DefaultSkillEngine::new(Box::new(source), vec![]);

        let result = engine
            .resolve_and_render(&[SkillId("nonexistent/skill".into())])
            .await;
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_preload_missing_skill_errors() {
        let source = InMemorySkillSource::new(vec![test_skill(
            "existing/skill",
            "Existing",
            &[],
        )]);
        let engine = DefaultSkillEngine::new(Box::new(source), vec![]);

        // One valid, one invalid — should fail on the invalid one
        let result = engine
            .resolve_and_render(&[
                SkillId("existing/skill".into()),
                SkillId("missing/skill".into()),
            ])
            .await;
        assert!(matches!(result, Err(SkillError::NotFound { .. })));
    }

    // --- list_skills ---

    #[tokio::test]
    async fn test_list_skills_no_filter() {
        let engine = DefaultSkillEngine::new(Box::new(multi_source()), vec![]);
        let skills = engine
            .list_skills(&SkillFilter::default())
            .await
            .unwrap();
        assert_eq!(skills.len(), 4);
    }

    #[tokio::test]
    async fn test_list_skills_collection_filter() {
        let engine = DefaultSkillEngine::new(Box::new(multi_source()), vec![]);
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

    #[tokio::test]
    async fn test_list_skills_query_filter() {
        let engine = DefaultSkillEngine::new(Box::new(multi_source()), vec![]);
        let skills = engine
            .list_skills(&SkillFilter {
                query: Some("email".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id.0, "extraction/email");
    }

    // --- collections ---

    #[tokio::test]
    async fn test_collections_derived() {
        let engine = DefaultSkillEngine::new(Box::new(multi_source()), vec![]);
        let collections = engine.collections().await.unwrap();

        // extraction (2 skills) and formatting (1 skill)
        assert_eq!(collections.len(), 2);
        let extraction = collections.iter().find(|c| c.path == "extraction");
        assert!(extraction.is_some());
        assert_eq!(extraction.map(|c| c.count), Some(2));
    }
}
