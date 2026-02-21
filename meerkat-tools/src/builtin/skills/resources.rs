//! Skill resource tools.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillId, SkillRuntime};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError};

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

fn canonical_key(id: &SkillId) -> Value {
    match id.0.split_once('/') {
        Some((source_uuid, skill_name)) => {
            json!({"source_uuid": source_uuid, "skill_name": skill_name})
        }
        None => json!({"source_uuid": Value::Null, "skill_name": id.0}),
    }
}

#[async_trait]
impl BuiltinTool for SkillListResourcesTool {
    fn name(&self) -> &'static str {
        "skill_list_resources"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_list_resources".into(),
            description: "List resources/artifacts exposed by a skill.".into(),
            input_schema: crate::schema::schema_for::<SkillListResourcesArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let id_str = args.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            BuiltinToolError::InvalidArgs("Missing required 'id' parameter".into())
        })?;

        let id = SkillId(id_str.to_string());
        let artifacts = self
            .engine
            .list_artifacts(&id)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        Ok(json!({
            "id": id.0,
            "canonical_key": canonical_key(&id),
            "artifacts": artifacts,
        }))
    }
}

#[async_trait]
impl BuiltinTool for SkillReadResourceTool {
    fn name(&self) -> &'static str {
        "skill_read_resource"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_read_resource".into(),
            description: "Read a resource/artifact exposed by a skill.".into(),
            input_schema: crate::schema::schema_for::<SkillReadResourceArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
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

        Ok(json!({
            "id": id.0,
            "canonical_key": canonical_key(&id),
            "artifact": artifact,
        }))
    }
}
