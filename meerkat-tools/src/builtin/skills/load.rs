//! Load skill tool — activates a skill mid-turn.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::skills::{SkillEngine, SkillId};
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Arguments for the load_skill tool (used for schema generation).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct LoadSkillArgs {
    /// Canonical skill ID (e.g. "extraction/email-extractor").
    id: String,
}

/// Tool for loading a skill's full instructions into the conversation.
pub struct LoadSkillTool {
    engine: Arc<dyn SkillEngine>,
}

impl LoadSkillTool {
    pub fn new(engine: Arc<dyn SkillEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl BuiltinTool for LoadSkillTool {
    fn name(&self) -> &'static str {
        "load_skill"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "load_skill".into(),
            description: "Load a skill's full instructions into the conversation.".into(),
            input_schema: crate::schema::schema_for::<LoadSkillArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Enabled conditionally when skills are active
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let id_str = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BuiltinToolError::InvalidArgs("Missing required 'id' parameter".into())
            })?;

        let skill_id = SkillId(id_str.to_string());
        let results = self
            .engine
            .resolve_and_render(&[skill_id])
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        match results.into_iter().next() {
            Some(resolved) => Ok(json!({
                "id": resolved.id.0,
                "name": resolved.name,
                "body": resolved.rendered_body,
                "byte_size": resolved.byte_size,
            })),
            None => Err(BuiltinToolError::ExecutionFailed(
                "Skill resolved but returned no content".into(),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::{
        ResolvedSkill, SkillCollection, SkillDescriptor, SkillError, SkillFilter, SkillScope,
    };

    struct MockEngine {
        skills: Vec<SkillDescriptor>,
    }

    #[async_trait]
    impl SkillEngine for MockEngine {
        async fn inventory_section(&self) -> Result<String, SkillError> {
            Ok(String::new())
        }

        async fn resolve_and_render(
            &self,
            ids: &[SkillId],
        ) -> Result<Vec<ResolvedSkill>, SkillError> {
            let mut results = Vec::new();
            for id in ids {
                if let Some(skill) = self.skills.iter().find(|s| &s.id == id) {
                    results.push(ResolvedSkill {
                        id: id.clone(),
                        name: skill.name.clone(),
                        rendered_body: format!(
                            "<skill id=\"{}\">Body content</skill>",
                            id.0
                        ),
                        byte_size: 30,
                    });
                } else {
                    return Err(SkillError::NotFound { id: id.clone() });
                }
            }
            Ok(results)
        }

        async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
            Ok(vec![])
        }

        async fn list_skills(
            &self,
            _filter: &SkillFilter,
        ) -> Result<Vec<SkillDescriptor>, SkillError> {
            Ok(self.skills.clone())
        }
    }

    fn test_engine() -> Arc<dyn SkillEngine> {
        Arc::new(MockEngine {
            skills: vec![SkillDescriptor {
                id: SkillId("extraction/email".into()),
                name: "email".into(),
                description: "Extract from emails".into(),
                scope: SkillScope::Builtin,
                ..Default::default()
            }],
        })
    }

    #[tokio::test]
    async fn test_load_skill_returns_body() {
        let tool = LoadSkillTool::new(test_engine());
        let result = tool
            .call(json!({"id": "extraction/email"}))
            .await
            .unwrap();

        assert_eq!(result["id"], "extraction/email");
        assert_eq!(result["name"], "email");
        assert!(result["body"].as_str().unwrap().contains("Body content"));
        assert!(result["byte_size"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_load_skill_not_found() {
        let tool = LoadSkillTool::new(test_engine());
        let result = tool
            .call(json!({"id": "nonexistent/skill"}))
            .await;

        assert!(result.is_err());
    }
}
