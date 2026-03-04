use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::io::AsyncWriteExt;

use meerkat_mob::ids::{FlowId, MeerkatId, MobId, ProfileName};
use meerkat_mob::{MobRunStatus, SpawnMemberSpec, StepRunStatus};

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

    let context = input.context.as_deref().unwrap_or("");
    let overrides = input.model_overrides.unwrap_or_default();
    let total_steps = pack.flow_step_count();
    let definition = pack.definition(&input.task, context, &overrides);
    let mob_id = definition.id.clone();

    // Collect profile names before moving definition
    let profile_names: Vec<String> = definition.profiles.keys().map(|p| p.to_string()).collect();

    // Create mob
    state
        .mob_state
        .mob_create_definition(definition)
        .await
        .map_err(|e| ToolCallError::internal(format!("Mob creation failed: {e}")))?;

    // Spawn one agent per profile (meerkat_id = profile name for simplicity)
    let specs: Vec<SpawnMemberSpec> = profile_names
        .iter()
        .map(|name| SpawnMemberSpec {
            profile_name: ProfileName::from(name.as_str()),
            meerkat_id: MeerkatId::from(name.as_str()),
            initial_message: None,
            runtime_mode: None, // use profile default
            backend: None,
            context: None,
            labels: None,
            resume_session_id: None,
            additional_instructions: None,
        })
        .collect();

    let spawn_results = state
        .mob_state
        .mob_spawn_many(&mob_id, specs)
        .await
        .map_err(|e| ToolCallError::internal(format!("Spawn failed: {e}")))?;

    // Check for individual spawn failures
    let mut spawn_ok = 0;
    for (i, result) in spawn_results.iter().enumerate() {
        match result {
            Ok(_) => { tracing::info!(profile = %profile_names[i], "spawned"); spawn_ok += 1; },
            Err(e) => tracing::error!(profile = %profile_names[i], error = %e, "spawn failed"),
        }
    }
    tracing::info!(spawned = spawn_ok, total = profile_names.len(), "spawn complete");

    // Brief yield to let actor process spawn roster updates
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Ensure cleanup on all exit paths
    let result = run_flow(state, &mob_id, total_steps, progress_token.as_ref()).await;

    // Destroy mob (best-effort)
    if let Err(e) = state.mob_state.mob_destroy(&mob_id).await {
        tracing::warn!(mob_id = %mob_id, error = %e, "mob cleanup failed");
    }

    result
}

async fn run_flow(
    state: &ForceState,
    mob_id: &MobId,
    total_steps: usize,
    progress_token: Option<&Value>,
) -> Result<Value, ToolCallError> {
    let flow_id = FlowId::from("main");

    // Send initial progress
    if let Some(token) = progress_token {
        send_progress(token, 0, total_steps, "starting flow").await;
    }

    let run_id = state
        .mob_state
        .mob_run_flow(mob_id, flow_id, json!({}))
        .await
        .map_err(|e| ToolCallError::internal(format!("Flow start failed: {e}")))?;

    let mut last_completed = 0usize;

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

        // Track completed + in-progress steps for progress reporting
        let completed = run
            .step_ledger
            .iter()
            .filter(|e| e.status == StepRunStatus::Completed)
            .count();
        let in_progress: Vec<_> = run
            .step_ledger
            .iter()
            .filter(|e| e.status == StepRunStatus::Dispatched)
            .map(|e| e.step_id.to_string())
            .collect();

        if completed > last_completed || !in_progress.is_empty() {
            if completed > last_completed {
                last_completed = completed;
            }
            if let Some(token) = progress_token {
                let label = if !in_progress.is_empty() {
                    in_progress.join(", ")
                } else {
                    "waiting".into()
                };
                send_progress(token, completed, total_steps, &label).await;
            }
        }

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
                    .find(|e| e.status == StepRunStatus::Completed)
                    .and_then(|e| e.output.as_ref());

                // With StepOutputFormat::Text, output is stored as Value::String
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
                    if errors.is_empty() { "unknown error".into() } else { errors.join("; ") }
                )));
            }
            MobRunStatus::Canceled => {
                return Err(ToolCallError::internal("Flow was canceled".to_string()));
            }
            _ => continue,
        }
    }
}

/// Send an MCP progress notification to stdout.
async fn send_progress(token: &Value, progress: usize, total: usize, label: &str) {
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "progress": progress,
            "total": total,
            "message": label,
        }
    });
    // Write directly to stdout — this is safe because the main loop is awaiting
    // the tool handler, so no concurrent writes.
    let mut stdout = tokio::io::stdout();
    let msg = format!("{notification}\n");
    let _ = stdout.write_all(msg.as_bytes()).await;
    let _ = stdout.flush().await;
}
