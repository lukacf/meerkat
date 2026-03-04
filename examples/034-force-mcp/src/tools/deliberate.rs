use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use meerkat_mob::ids::{FlowId, MobId};
use meerkat_mob::MobRunStatus;

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
    _progress_token: Option<Value>,
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

    let context = input.context.as_deref().unwrap_or("");
    let overrides = input.model_overrides.unwrap_or_default();
    let definition = pack.definition(&input.task, context, &overrides);
    let mob_id = definition.id.clone();

    // Create mob
    state
        .mob_state
        .mob_create_definition(definition)
        .await
        .map_err(|e| ToolCallError::internal(format!("Mob creation failed: {e}")))?;

    // Ensure cleanup on all exit paths
    let result = run_flow(state, &mob_id, pack.flow_step_count()).await;

    // Destroy mob (best-effort)
    if let Err(e) = state.mob_state.mob_destroy(&mob_id).await {
        tracing::warn!(mob_id = %mob_id, error = %e, "mob cleanup failed");
    }

    result
}

async fn run_flow(
    state: &ForceState,
    mob_id: &MobId,
    _total_steps: usize,
) -> Result<Value, ToolCallError> {
    let flow_id = FlowId::from("main");

    let run_id = state
        .mob_state
        .mob_run_flow(mob_id, flow_id, json!({}))
        .await
        .map_err(|e| ToolCallError::internal(format!("Flow start failed: {e}")))?;

    // Poll for completion
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let mob_run = state
            .mob_state
            .mob_flow_status(mob_id, run_id.clone())
            .await
            .map_err(|e| ToolCallError::internal(format!("Flow status failed: {e}")))?;

        let Some(run) = mob_run else {
            continue;
        };

        if !run.status.is_terminal() {
            continue;
        }

        // Extract result from the last completed step
        match run.status {
            MobRunStatus::Completed => {
                let last_output = run
                    .step_ledger
                    .iter()
                    .rev()
                    .find(|e| e.status == meerkat_mob::StepRunStatus::Completed)
                    .and_then(|e| e.output.as_ref());

                let text = match last_output {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
                    None => "Flow completed but produced no output.".to_string(),
                };

                return Ok(json!({
                    "content": [{"type": "text", "text": text}]
                }));
            }
            MobRunStatus::Failed => {
                let errors: Vec<String> = run
                    .failure_ledger
                    .iter()
                    .map(|f| format!("{}: {}", f.step_id, f.reason))
                    .collect();
                return Err(ToolCallError::internal(format!(
                    "Flow failed: {}",
                    errors.join("; ")
                )));
            }
            MobRunStatus::Canceled => {
                return Err(ToolCallError::internal("Flow was canceled".to_string()));
            }
            _ => continue,
        }
    }
}
