//! Skill resource tools.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillId, SkillRuntime};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct SkillListResourcesArgs {
    id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct SkillReadResourceArgs {
    id: String,
    path: String,
}

pub struct SkillListResourcesTool {
    engine: Arc<SkillRuntime>,
}

impl SkillListResourcesTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
}

pub struct SkillReadResourceTool {
    engine: Arc<SkillRuntime>,
}

impl SkillReadResourceTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for SkillListResourcesTool {
    fn name(&self) -> &'static str {
        "skill_list_resources"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_list_resources".into(),
            description: "List resources and artifacts exposed by a skill.\n\nResources are supplementary files bundled with a skill — templates, example data, configuration files, reference documents, etc. Use this to discover what is available before reading specific resources with skill_read_resource.\n\nThe id must be a canonical skill ID from browse_skills (e.g. \"extraction/email\").\n\nExample:\n  skill_list_resources {\"id\": \"extraction/email\"}\n  Returns: {\"id\": \"extraction/email\", \"artifacts\": [{\"path\": \"templates/default.txt\", \"description\": \"Default extraction template\", \"size_bytes\": 512}, {\"path\": \"examples/invoice.json\", \"description\": \"Sample invoice extraction\", \"size_bytes\": 1024}]}".into(),
            input_schema: crate::schema::schema_for::<SkillListResourcesArgs>(),
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
        let id_str = args.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            BuiltinToolError::InvalidArgs("Missing required 'id' parameter".into())
        })?;

        let id = SkillId(id_str.to_string());
        let artifacts = self
            .engine
            .list_artifacts(&id)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::Json(json!({
            "id": id.0,
            "artifacts": artifacts,
        })))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for SkillReadResourceTool {
    fn name(&self) -> &'static str {
        "skill_read_resource"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_read_resource".into(),
            description: "Read the content of a specific resource/artifact exposed by a skill.\n\nUse skill_list_resources first to discover available artifact paths, then read specific ones. Resources can be templates, example data, configuration, or any supplementary content bundled with the skill.\n\nParameters:\n- id: Canonical skill ID (e.g. \"extraction/email\").\n- path: Artifact path from skill_list_resources (e.g. \"templates/default.txt\").\n\nExample:\n  skill_read_resource {\"id\": \"extraction/email\", \"path\": \"templates/default.txt\"}\n  Returns: {\"id\": \"extraction/email\", \"artifact\": {\"path\": \"templates/default.txt\", \"content\": \"From: {{sender}}\\nSubject: {{subject}}\\n...\"}}".into(),
            input_schema: crate::schema::schema_for::<SkillReadResourceArgs>(),
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
        let id_str = args.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            BuiltinToolError::InvalidArgs("Missing required 'id' parameter".into())
        })?;
        let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            BuiltinToolError::InvalidArgs("Missing required 'path' parameter".into())
        })?;

        let id = SkillId(id_str.to_string());
        let artifact = self
            .engine
            .read_artifact(&id, path)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::Json(json!({
            "id": id.0,
            "artifact": artifact,
        })))
    }
}
