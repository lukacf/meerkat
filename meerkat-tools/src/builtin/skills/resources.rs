//! Skill resource tools.
//!
//! Keyed by typed `SkillKey` (source_uuid + skill_name) — no slash-string
//! parsing anywhere on the ingress path.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillKey, SkillName, SkillRuntime, SourceUuid};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillListResourcesArgs {
    source_uuid: String,
    skill_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillReadResourceArgs {
    source_uuid: String,
    skill_name: String,
    path: String,
}

fn parse_key(source_raw: &str, skill_raw: &str) -> Result<SkillKey, BuiltinToolError> {
    let source_uuid =
        SourceUuid::parse(source_raw).map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
    let skill_name =
        SkillName::parse(skill_raw).map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
    Ok(SkillKey {
        source_uuid,
        skill_name,
    })
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
            description:
                "List resources exposed by a skill identified by (source_uuid, skill_name).".into(),
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
        let args: SkillListResourcesArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.source_uuid, &args.skill_name)?;
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let artifacts = self
            .engine
            .list_artifacts(&key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolOutput::Json(json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
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
            description:
                "Read a resource at `path` from a skill identified by (source_uuid, skill_name)."
                    .into(),
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
        let args: SkillReadResourceArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.source_uuid, &args.skill_name)?;
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let artifact = self
            .engine
            .read_artifact(&key, &args.path)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolOutput::Json(json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
            "artifact": artifact,
        })))
    }
}
