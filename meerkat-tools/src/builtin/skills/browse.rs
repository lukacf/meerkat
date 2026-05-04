//! Browse skills tool — lists skills with optional search.
//!
//! Post-V4 the tool keys every listed skill by `SkillKey` (source_uuid +
//! skill_name) rendered as a structured JSON object, not a slash-delimited
//! path. Ingress and egress both go through the typed `SkillKey` — no
//! string parsing of identifiers.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillFilter, SkillRuntime, SourceUuid};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct BrowseSkillsArgs {
    /// Free-text search across skill names and descriptions.
    #[serde(default)]
    query: Option<String>,
    /// Restrict to a single source UUID.
    #[serde(default)]
    source_uuid: Option<String>,
}

pub struct BrowseSkillsTool {
    engine: Arc<SkillRuntime>,
}

impl BrowseSkillsTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for BrowseSkillsTool {
    fn name(&self) -> &'static str {
        "browse_skills"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "browse_skills".into(),
            description:
                "List available skills, optionally filtered by search query or source UUID.".into(),
            input_schema: crate::schema::schema_for::<BrowseSkillsArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "skills".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: BrowseSkillsArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let source_uuid = match args.source_uuid.as_deref() {
            Some(raw) => Some(
                SourceUuid::parse(raw)
                    .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?,
            ),
            None => None,
        };

        let filter = SkillFilter {
            query: args.query,
            source_uuid,
        };

        let entries = self
            .engine
            .list_all_with_provenance(&filter)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        let skill_values: Vec<Value> = entries
            .iter()
            .filter(|entry| entry.is_active)
            .map(|entry| {
                let s = &entry.descriptor;
                json!({
                    "source_uuid": s.key.source_uuid.to_string(),
                    "skill_name": s.key.skill_name.as_str(),
                    "name": s.name,
                    "description": s.description,
                })
            })
            .collect();

        Ok(ToolOutput::Json(json!({
            "skills": skill_values,
        })))
    }
}
