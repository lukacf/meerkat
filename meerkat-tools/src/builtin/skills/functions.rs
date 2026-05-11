//! Skill function tool.
//!
//! Keyed by typed `SkillKey` objects — no decomposed sibling fields or
//! slash-string parsing of skill identity on the ingress path.

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
struct SkillInvokeFunctionArgs {
    skill_key: SkillKeyInput,
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for SkillInvokeFunctionTool {
    fn name(&self) -> &'static str {
        "skill_invoke_function"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "skill_invoke_function".into(),
            description: "Invoke a function exposed by a skill identified by canonical skill_key."
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
        let args: SkillInvokeFunctionArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.skill_key)?;
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let output = self
            .engine
            .invoke_function(&key, &args.function_name, args.arguments)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::Json(json!({
            "skill_key": &key,
            "function_name": args.function_name,
            "output": output,
        })))
    }
}
