//! Skill function tool.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{SkillId, SkillRuntime};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct SkillInvokeFunctionArgs {
    id: String,
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

fn canonical_key(id: &SkillId) -> Value {
    match id.0.split_once('/') {
        Some((source_uuid, skill_name)) => {
            json!({"source_uuid": source_uuid, "skill_name": skill_name})
        }
        None => json!({"source_uuid": Value::Null, "skill_name": id.0}),
    }
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
            description: "Invoke a function exposed by a skill.".into(),
            input_schema: crate::schema::schema_for::<SkillInvokeFunctionArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let id_str = args.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            BuiltinToolError::InvalidArgs("Missing required 'id' parameter".into())
        })?;
        let function_name = args
            .get("function_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BuiltinToolError::InvalidArgs("Missing required 'function_name' parameter".into())
            })?;

        let id = SkillId(id_str.to_string());
        let output = self
            .engine
            .invoke_function(
                &id,
                function_name,
                args.get("arguments").cloned().unwrap_or(Value::Null),
            )
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;

        Ok(json!({
            "id": id.0,
            "canonical_key": canonical_key(&id),
            "function_name": function_name,
            "output": output,
        }))
    }
}
