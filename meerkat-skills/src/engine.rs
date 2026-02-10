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
}

impl DefaultSkillEngine {
    pub fn new(source: Box<dyn SkillSource>, available_capabilities: Vec<String>) -> Self {
        Self {
            source,
            available_capabilities: available_capabilities.into_iter().collect(),
        }
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
            renderer::DEFAULT_INVENTORY_THRESHOLD,
        ))
    }

    async fn resolve_and_render(
        &self,
        ids: &[SkillId],
    ) -> Result<Vec<ResolvedSkill>, SkillError> {
        let mut results = Vec::new();
        for id in ids {
            let doc = self.source.load(id).await?;
            let rendered = renderer::render_injection(&id.0, &doc.body);
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

    #[tokio::test]
    async fn test_inventory_section() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        let engine = DefaultSkillEngine::new(
            Box::new(source),
            vec!["builtins".to_string(), "shell".to_string()],
        );

        let section = engine.inventory_section().await.unwrap();
        // Now uses XML format
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

        // Only builtins available — shell-patterns should be filtered out
        let engine =
            DefaultSkillEngine::new(Box::new(source), vec!["builtins".to_string()]);

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("<skill id=\"task-workflow\">"));
        assert!(!section.contains("shell-patterns"));
    }

    #[tokio::test]
    async fn test_resolve_and_render() {
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
        // Now uses <skill id="..."> format
        assert!(result[0]
            .rendered_body
            .contains("<skill id=\"task-workflow\">"));
        assert!(result[0].rendered_body.contains("Content for task-workflow"));
    }
}
