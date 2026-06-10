//! Skill function tool.
//!
//! Keyed by typed `SkillKey` (source_uuid + skill_name) — no slash-string
//! parsing of skill identity on the ingress path. Function identity and
//! arguments are parsed fail-closed at this ingress into the typed core
//! contract (`SkillFunctionName` + `ToolCallArguments`); the engine seam
//! never sees a bare `&str` function name or an unvalidated JSON bag.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolCallArguments;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillFunctionName, SkillKey, SkillName, SkillRuntime, SourceUuid};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillInvokeFunctionArgs {
    source_uuid: String,
    skill_name: String,
    function_name: String,
    /// Structured function arguments. Must be a JSON object when present;
    /// non-object shapes are rejected at ingress (`ToolCallArguments`
    /// fail-closed parse), never ferried to the engine.
    #[serde(default)]
    #[schemars(with = "Option<std::collections::BTreeMap<String, serde_json::Value>>")]
    arguments: Option<ToolCallArguments>,
}

/// Typed response payload for a successful skill function invocation. The
/// function output itself is wire-opaque third-party JSON and is carried
/// verbatim (`RawValue`), never re-interpreted here.
#[derive(Debug, Serialize)]
struct SkillInvokeFunctionResponse {
    source_uuid: String,
    skill_name: String,
    function_name: String,
    output: Box<serde_json::value::RawValue>,
}

pub struct SkillInvokeFunctionTool {
    engine: Arc<SkillRuntime>,
}

impl SkillInvokeFunctionTool {
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
        let args: SkillInvokeFunctionArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.source_uuid, &args.skill_name)?;
        let function_name = SkillFunctionName::parse(&args.function_name)
            .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
        let arguments = args.arguments.unwrap_or_else(ToolCallArguments::empty);
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let output = self
            .engine
            .invoke_function(&key, &function_name, arguments)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        let response = SkillInvokeFunctionResponse {
            source_uuid: key.source_uuid.to_string(),
            skill_name: key.skill_name.as_str().to_string(),
            function_name: function_name.to_string(),
            output: output.into_raw(),
        };
        let value = serde_json::to_value(&response).map_err(|err| {
            BuiltinToolError::ExecutionFailed(format!(
                "failed to serialize skill function response: {err}"
            ))
        })?;
        Ok(ToolOutput::Json(value))
    }
}
