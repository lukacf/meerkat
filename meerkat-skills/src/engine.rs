//! Default skill engine implementation.

use std::collections::HashSet;

use async_trait::async_trait;
use meerkat_core::skills::{SkillDescriptor, SkillEngine, SkillError, SkillSource};

use crate::renderer;
use crate::resolver;

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
        let all_skills = self.source.list().await?;
        let available = filter_by_capabilities(all_skills, &self.available_capabilities);
        Ok(renderer::render_inventory(&available))
    }

    async fn resolve_and_render(
        &self,
        references: &[String],
        available_capabilities: &[String],
    ) -> Result<String, SkillError> {
        let all_skills = self.source.list().await?;
        let caps: HashSet<String> = available_capabilities.iter().cloned().collect();
        let available = filter_by_capabilities(all_skills, &caps);

        let mut blocks = Vec::new();
        for reference in references {
            match resolver::resolve_reference(reference, &available) {
                Ok(id) => {
                    let doc = self.source.load(&id).await?;
                    blocks.push(renderer::render_injection(&doc.descriptor.name, &doc.body));
                }
                Err(e) => return Err(e),
            }
        }

        Ok(blocks.join("\n\n"))
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
        assert!(section.contains("`/task-workflow`"));
        assert!(section.contains("`/shell-patterns`"));
    }

    #[tokio::test]
    async fn test_inventory_filters_by_capabilities() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &["builtins"]),
            test_skill("shell-patterns", "Shell Patterns", &["builtins", "shell"]),
        ]);

        // Only builtins available — shell-patterns should be filtered out
        let engine = DefaultSkillEngine::new(
            Box::new(source),
            vec!["builtins".to_string()],
        );

        let section = engine.inventory_section().await.unwrap();
        assert!(section.contains("`/task-workflow`"));
        assert!(!section.contains("`/shell-patterns`"));
    }

    #[tokio::test]
    async fn test_resolve_and_render() {
        let source = InMemorySkillSource::new(vec![
            test_skill("task-workflow", "Task Workflow", &[]),
        ]);

        let engine = DefaultSkillEngine::new(Box::new(source), vec![]);

        let result = engine
            .resolve_and_render(&["/task-workflow".to_string()], &[])
            .await
            .unwrap();
        assert!(result.contains("<skill name=\"Task Workflow\">"));
        assert!(result.contains("Content for task-workflow"));
    }
}
