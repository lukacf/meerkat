use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::state::ForceState;
use super::ToolCallError;

#[derive(Deserialize)]
struct DeliberateInput {
    pack: String,
    task: String,
    context: Option<String>,
    model_overrides: Option<BTreeMap<String, String>>,
}

pub async fn handle(
    state: &ForceState,
    arguments: &Value,
    progress_token: Option<Value>,
) -> Result<Value, ToolCallError> {
    let input: DeliberateInput = serde_json::from_value(arguments.clone())
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;

    let pack = state
        .pack_registry
        .get(&input.pack)
        .ok_or_else(|| {
            ToolCallError::invalid_params(format!(
                "Unknown pack: '{}'. Available: {}",
                input.pack,
                state.pack_registry.list_names().join(", ")
            ))
        })?;

    // TODO: Phase 4 — build MobDefinition from pack, create mob, run flow, return result
    let _ = (pack, input.task, input.context, input.model_overrides, progress_token);

    Err(ToolCallError::internal("deliberate not yet implemented"))
}
