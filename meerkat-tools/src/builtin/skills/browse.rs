//! Browse skills tool — lists collections and skills.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::skills::{SkillCollection, SkillDescriptor, SkillEngine, SkillFilter};
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Arguments for the browse_skills tool (used for schema generation).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct BrowseSkillsArgs {
    /// Collection path to browse (e.g. "extraction"). Omit to browse root.
    #[serde(default)]
    path: Option<String>,
    /// Search across skill names and descriptions.
    #[serde(default)]
    query: Option<String>,
}

/// Tool for browsing skill collections and skills.
///
/// Returns a discriminated response:
/// - `{ "type": "listing", "path": "...", "subcollections": [...], "skills": [...] }`
/// - `{ "type": "search", "query": "...", "skills": [...] }`
pub struct BrowseSkillsTool {
    engine: Arc<dyn SkillEngine>,
}

impl BrowseSkillsTool {
    pub fn new(engine: Arc<dyn SkillEngine>) -> Self {
        Self { engine }
    }
}

/// Partition skills into direct children and subcollections at the given path.
fn partition_at_path(
    path: &str,
    skills: &[SkillDescriptor],
    all_collections: &[SkillCollection],
) -> (Vec<Value>, Vec<Value>) {
    // Direct skills: those whose collection path == the browsed path exactly
    let direct_skills: Vec<Value> = skills
        .iter()
        .filter(|s| {
            let coll = s.id.collection().unwrap_or("");
            coll == path
        })
        .map(|s| {
            json!({
                "id": s.id.0,
                "name": s.name,
                "description": s.description,
            })
        })
        .collect();

    // Subcollections: collections that are immediate children of the browsed path
    let subcollections: Vec<Value> = all_collections
        .iter()
        .filter(|c| {
            if path.is_empty() {
                // Root level: top-level collections (no `/` in path)
                !c.path.contains('/')
            } else {
                // Nested: collection path starts with `{path}/` and has exactly one more segment
                c.path.starts_with(path)
                    && c.path.len() > path.len()
                    && c.path.as_bytes().get(path.len()) == Some(&b'/')
                    && !c.path[path.len() + 1..].contains('/')
            }
        })
        .map(|c| {
            json!({
                "path": c.path,
                "description": c.description,
                "count": c.count,
            })
        })
        .collect();

    (subcollections, direct_skills)
}

#[async_trait]
impl BuiltinTool for BrowseSkillsTool {
    fn name(&self) -> &'static str {
        "browse_skills"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "browse_skills".into(),
            description: "Browse available skill collections or search for skills.".into(),
            input_schema: crate::schema::schema_for::<BrowseSkillsArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Enabled conditionally when skills are active
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let query = args.get("query").and_then(|v| v.as_str());
        let path = args.get("path").and_then(|v| v.as_str());

        // Query wins over path (as per spec)
        if let Some(query_str) = query {
            let filter = SkillFilter {
                query: Some(query_str.to_string()),
                ..Default::default()
            };
            let skills = self
                .engine
                .list_skills(&filter)
                .await
                .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

            let skill_values: Vec<Value> = skills
                .iter()
                .map(|s| {
                    json!({
                        "id": s.id.0,
                        "name": s.name,
                        "description": s.description,
                    })
                })
                .collect();

            return Ok(json!({
                "type": "search",
                "query": query_str,
                "skills": skill_values,
            }));
        }

        // Listing mode
        let browse_path = path.unwrap_or("");
        let filter = if browse_path.is_empty() {
            SkillFilter::default()
        } else {
            SkillFilter {
                collection: Some(browse_path.to_string()),
                ..Default::default()
            }
        };

        let skills = self
            .engine
            .list_skills(&filter)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let collections = self
            .engine
            .collections()
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        let (subcollections, direct_skills) =
            partition_at_path(browse_path, &skills, &collections);

        Ok(json!({
            "type": "listing",
            "path": browse_path,
            "subcollections": subcollections,
            "skills": direct_skills,
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::skills::{
        ResolvedSkill, SkillCollection, SkillDescriptor, SkillError, SkillId, SkillScope,
    };

    struct MockEngine {
        skills: Vec<SkillDescriptor>,
    }

    impl MockEngine {
        fn new(skills: Vec<(&str, &str)>) -> Self {
            Self {
                skills: skills
                    .into_iter()
                    .map(|(id, name)| SkillDescriptor {
                        id: SkillId(id.into()),
                        name: name.into(),
                        description: format!("Description for {name}"),
                        scope: SkillScope::Builtin,
                        ..Default::default()
                    })
                    .collect(),
            }
        }
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
                        rendered_body: format!("<skill id=\"{}\">body</skill>", id.0),
                        byte_size: 20,
                    });
                } else {
                    return Err(SkillError::NotFound { id: id.clone() });
                }
            }
            Ok(results)
        }

        async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
            Ok(meerkat_core::skills::derive_collections(&self.skills))
        }

        async fn list_skills(
            &self,
            filter: &SkillFilter,
        ) -> Result<Vec<SkillDescriptor>, SkillError> {
            Ok(meerkat_core::skills::apply_filter(&self.skills, filter))
        }
    }

    fn test_engine() -> Arc<dyn SkillEngine> {
        Arc::new(MockEngine::new(vec![
            ("extraction/email", "email"),
            ("extraction/fiction", "fiction"),
            ("formatting/markdown", "markdown"),
            ("pdf-processing", "pdf-processing"),
        ]))
    }

    #[tokio::test]
    async fn test_browse_root_returns_listing() {
        let tool = BrowseSkillsTool::new(test_engine());
        let result = tool.call(json!({})).await.unwrap();

        assert_eq!(result["type"], "listing");
        assert_eq!(result["path"], "");
        assert!(result["subcollections"].is_array());
        assert!(result["skills"].is_array());

        // Root has subcollections: extraction, formatting
        let subs = result["subcollections"].as_array().unwrap();
        assert!(subs.iter().any(|s| s["path"] == "extraction"));

        // Root has direct skill: pdf-processing
        let skills = result["skills"].as_array().unwrap();
        assert!(skills.iter().any(|s| s["id"] == "pdf-processing"));
    }

    #[tokio::test]
    async fn test_browse_collection_returns_listing() {
        let tool = BrowseSkillsTool::new(test_engine());
        let result = tool.call(json!({"path": "extraction"})).await.unwrap();

        assert_eq!(result["type"], "listing");
        assert_eq!(result["path"], "extraction");

        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|s| s["id"] == "extraction/email"));
        assert!(skills.iter().any(|s| s["id"] == "extraction/fiction"));
    }

    #[tokio::test]
    async fn test_browse_search_returns_search() {
        let tool = BrowseSkillsTool::new(test_engine());
        let result = tool.call(json!({"query": "email"})).await.unwrap();

        assert_eq!(result["type"], "search");
        assert_eq!(result["query"], "email");

        let skills = result["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["id"], "extraction/email");
    }

    #[tokio::test]
    async fn test_browse_both_query_wins() {
        let tool = BrowseSkillsTool::new(test_engine());
        let result = tool
            .call(json!({"path": "formatting", "query": "email"}))
            .await
            .unwrap();

        // Query wins — returns search mode, not listing
        assert_eq!(result["type"], "search");
        assert_eq!(result["query"], "email");
    }

    #[tokio::test]
    async fn test_browse_empty_collection() {
        let tool = BrowseSkillsTool::new(test_engine());
        let result = tool
            .call(json!({"path": "nonexistent"}))
            .await
            .unwrap();

        assert_eq!(result["type"], "listing");
        let skills = result["skills"].as_array().unwrap();
        assert!(skills.is_empty());
    }
}
