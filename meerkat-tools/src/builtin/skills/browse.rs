//! Browse skills tool — lists collections and skills.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillCollection, SkillDescriptor, SkillFilter, SkillId, SkillRuntime};
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
    engine: Arc<SkillRuntime>,
}

impl BrowseSkillsTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
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
                "canonical_key": canonical_key(&s.id),
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

fn canonical_key(id: &SkillId) -> Value {
    match id.0.split_once('/') {
        Some((source_uuid, skill_name)) => {
            json!({ "source_uuid": source_uuid, "skill_name": skill_name })
        }
        None => json!({ "source_uuid": Value::Null, "skill_name": id.0 }),
    }
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
                        "canonical_key": canonical_key(&s.id),
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

        let (subcollections, direct_skills) = partition_at_path(browse_path, &skills, &collections);

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
        ResolvedSkill, SkillArtifact, SkillArtifactContent, SkillCollection, SkillDescriptor,
        SkillEngine, SkillError, SkillId, SkillScope,
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

    impl SkillEngine for MockEngine {
        fn inventory_section(
            &self,
        ) -> impl std::future::Future<Output = Result<String, SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            ids: &[SkillId],
        ) -> impl std::future::Future<Output = Result<Vec<ResolvedSkill>, SkillError>> + Send
        {
            let ids = ids.to_vec();
            async move {
                let mut results = Vec::new();
                for id in &ids {
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
        }

        fn collections(
            &self,
        ) -> impl std::future::Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send
        {
            async move { Ok(meerkat_core::skills::derive_collections(&self.skills)) }
        }

        fn list_skills(
            &self,
            filter: &SkillFilter,
        ) -> impl std::future::Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send
        {
            async move { Ok(meerkat_core::skills::apply_filter(&self.skills, filter)) }
        }

        fn quarantined_diagnostics(
            &self,
        ) -> impl std::future::Future<
            Output = Result<Vec<meerkat_core::skills::SkillQuarantineDiagnostic>, SkillError>,
        > + Send {
            async move { Ok(Vec::new()) }
        }

        fn health_snapshot(
            &self,
        ) -> impl std::future::Future<
            Output = Result<meerkat_core::skills::SourceHealthSnapshot, SkillError>,
        > + Send {
            async move { Ok(meerkat_core::skills::SourceHealthSnapshot::default()) }
        }

        fn list_artifacts(
            &self,
            _id: &SkillId,
        ) -> impl std::future::Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
        }

        fn read_artifact(
            &self,
            id: &SkillId,
            _artifact_path: &str,
        ) -> impl std::future::Future<Output = Result<SkillArtifactContent, SkillError>> + Send
        {
            let id = id.clone();
            async move { Err(SkillError::NotFound { id }) }
        }

        fn invoke_function(
            &self,
            id: &SkillId,
            _function_name: &str,
            _arguments: Value,
        ) -> impl std::future::Future<Output = Result<Value, SkillError>> + Send {
            let id = id.clone();
            async move { Err(SkillError::NotFound { id }) }
        }
    }

    fn test_engine() -> Arc<SkillRuntime> {
        Arc::new(SkillRuntime::new(Arc::new(MockEngine::new(vec![
            ("extraction/email", "email"),
            ("extraction/fiction", "fiction"),
            ("formatting/markdown", "markdown"),
            ("pdf-processing", "pdf-processing"),
        ]))))
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
        let result = tool.call(json!({"path": "nonexistent"})).await.unwrap();

        assert_eq!(result["type"], "listing");
        let skills = result["skills"].as_array().unwrap();
        assert!(skills.is_empty());
    }
}
