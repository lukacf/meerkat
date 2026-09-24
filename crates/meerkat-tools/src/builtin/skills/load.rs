//! Load skill tool — activates a skill mid-turn.
//!
//! The tool accepts a typed `SkillKey` on the wire (`source_uuid` +
//! `skill_name` JSON fields), not a slash-delimited path. The ingress
//! parser validates both halves before dispatching to the runtime.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillKey, SkillName, SkillRuntime, SourceUuid};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct LoadSkillArgs {
    source_uuid: String,
    skill_name: String,
}

/// Tool for loading a skill's full instructions into the conversation.
pub struct LoadSkillTool {
    engine: Arc<SkillRuntime>,
}

impl LoadSkillTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for LoadSkillTool {
    fn name(&self) -> &'static str {
        "load_skill"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "load_skill".into(),
            description:
                "Load a skill's full instructions by (source_uuid, skill_name) into the conversation."
                    .into(),
            input_schema: crate::schema::schema_for::<LoadSkillArgs>(),
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
        let args: LoadSkillArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.source_uuid, &args.skill_name)?;
        // Apply the source-identity lineage remap chain before dispatch
        // so legacy source_uuids that have since been rotated/merged
        // still resolve to the canonical backing skill.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let results = self
            .engine
            .resolve_and_render(std::slice::from_ref(&key))
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        match results.into_iter().next() {
            Some(resolved) => Ok(ToolOutput::Json(json!({
                "source_uuid": resolved.key.source_uuid.to_string(),
                "skill_name": resolved.key.skill_name.as_str(),
                "name": resolved.name,
                "body": resolved.rendered_body,
                "byte_size": resolved.byte_size,
            }))),
            None => Err(BuiltinToolError::ExecutionFailed(
                "Skill resolved but returned no content".into(),
            )),
        }
    }
}
