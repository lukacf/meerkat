//! Skill resource tools.
//!
//! Keyed by typed `SkillKey` objects — no decomposed sibling fields or
//! slash-string parsing anywhere on the ingress path.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillKey, SkillName, SkillRuntime, SourceUuid};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillKeyInput {
    source_uuid: String,
    skill_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillListResourcesArgs {
    skill_key: SkillKeyInput,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillReadResourceArgs {
    skill_key: SkillKeyInput,
    path: String,
}

fn parse_key(input: &SkillKeyInput) -> Result<SkillKey, BuiltinToolError> {
    let source_uuid = SourceUuid::parse(&input.source_uuid)
        .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
    let skill_name = SkillName::parse(&input.skill_name)
        .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
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
            description: "List resources exposed by a skill identified by canonical skill_key."
                .into(),
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
        let raw_key = parse_key(&args.skill_key)?;
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
            "skill_key": &key,
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
                "Read a resource at `path` from a skill identified by canonical skill_key.".into(),
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
        let raw_key = parse_key(&args.skill_key)?;
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
            "skill_key": &key,
            "artifact": artifact,
        })))
    }
}
