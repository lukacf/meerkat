//! Skill function tool.
//!
//! Keyed by typed `SkillKey` (source_uuid + skill_name) — no slash-string
//! parsing of skill identity on the ingress path.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillKey, SkillName, SkillRuntime, SourceUuid};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct SkillInvokeFunctionArgs {
    source_uuid: String,
    skill_name: String,
    function_name: String,
    #[serde(default)]
    arguments: Value,
}

pub struct SkillInvokeFunctionTool {
    engine: Arc<SkillRuntime>,
}

impl SkillInvokeFunctionTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
}

fn parse_key(args: &Value) -> Result<SkillKey, BuiltinToolError> {
    let source_raw = args
        .get("source_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BuiltinToolError::InvalidArgs("missing 'source_uuid'".into()))?;
    let skill_raw = args
        .get("skill_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BuiltinToolError::InvalidArgs("missing 'skill_name'".into()))?;
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
impl BuiltinTool for SkillInvokeFunctionTool {
    fn name(&self) -> &'static str {
        "skill_invoke_function"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_invoke_function".into(),
            description:
                "Invoke a function exposed by a skill identified by (source_uuid, skill_name)."
                    .into(),
            input_schema: crate::schema::schema_for::<SkillInvokeFunctionArgs>(),
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
        let raw_key = parse_key(&args)?;
        let function_name = args
            .get("function_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BuiltinToolError::InvalidArgs("missing 'function_name' parameter".into())
            })?;
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let output = self
            .engine
            .invoke_function(
                &key,
                function_name,
                args.get("arguments").cloned().unwrap_or(Value::Null),
            )
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::Json(json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
            "function_name": function_name,
            "output": output,
        })))
    }
}
