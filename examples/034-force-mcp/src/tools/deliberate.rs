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
    let has_flows = total_steps > 0;
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
            runtime_mode: None,
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

    for (i, result) in spawn_results.iter().enumerate() {
        match result {
            Ok(_) => tracing::info!(profile = %profile_names[i], "spawned"),
            Err(e) => tracing::error!(profile = %profile_names[i], error = %e, "spawn failed"),
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Route to flow-based or comms-based execution
    let result = if has_flows {
        run_flow(state, &mob_id, total_steps, progress_token.as_ref()).await
    } else {
        // Build the prompt with context
        let prompt = if context.is_empty() {
            input.task.clone()
        } else {
            format!("{}\n\n## Context\n\n{context}", input.task)
        };
        run_comms(state, &mob_id, &prompt, progress_token.as_ref()).await
    };

    // Destroy mob (best-effort)
    if let Err(e) = state.mob_state.mob_destroy(&mob_id).await {
        tracing::warn!(mob_id = %mob_id, error = %e, "mob cleanup failed");
    }

    result
}

// ── Flow-based execution (structured packs) ─────────────────────────────────

async fn run_flow(
    state: &ForceState,
    mob_id: &MobId,
    total_steps: usize,
    progress_token: Option<&Value>,
) -> Result<Value, ToolCallError> {
    let flow_id = FlowId::from("main");

    if let Some(token) = progress_token {
        send_progress(token, 0, total_steps, "starting flow").await;
    }

    let run_id = state
        .mob_state
        .mob_run_flow(mob_id, flow_id, json!({}))
        .await
        .map_err(|e| ToolCallError::internal(format!("Flow start failed: {e}")))?;

    let mut last_completed = 0usize;

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let mob_run = state
            .mob_state
            .mob_flow_status(mob_id, run_id.clone())
            .await
            .map_err(|e| ToolCallError::internal(format!("Flow status failed: {e}")))?;

        let Some(run) = mob_run else { continue };

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

        match run.status {
            MobRunStatus::Completed => {
                let last_output = run
                    .step_ledger
                    .iter()
                    .rev()
                    .find(|e| e.status == StepRunStatus::Completed)
                    .and_then(|e| e.output.as_ref());

                let text = match last_output {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
                    None => "Flow completed but produced no output.".to_string(),
                };

                return Ok(json!({"content": [{"type": "text", "text": text}]}));
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

// ── Comms-based execution (autonomous panel) ────────────────────────────────

async fn run_comms(
    state: &ForceState,
    mob_id: &MobId,
    task: &str,
    progress_token: Option<&Value>,
) -> Result<Value, ToolCallError> {
    // Send the task to the moderator (orchestrator) — kicks off the discussion
    let moderator_id = MeerkatId::from("moderator");

    if let Some(token) = progress_token {
        send_progress(token, 0, 1, "panel deliberating").await;
    }

    state
        .mob_state
        .mob_send_message(mob_id, moderator_id, task.to_string())
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to trigger moderator: {e}")))?;

    // Poll mob events until quiescence (no new events for QUIET_THRESHOLD)
    const MAX_WAIT_MS: u64 = 300_000; // 5 min max
    const QUIET_THRESHOLD_MS: u64 = 15_000; // 15s of quiet = done
    const POLL_INTERVAL_MS: u64 = 500;

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(MAX_WAIT_MS);
    let mut last_event_time = tokio::time::Instant::now();
    let mut event_cursor = 0u64;
    let mut last_moderator_text = String::new();
    let mut total_events = 0u64;

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;

        if tokio::time::Instant::now() > deadline {
            tracing::warn!("panel hit max wait time");
            break;
        }

        // Poll mob events
        let events = state
            .mob_state
            .mob_events(mob_id, event_cursor, 100)
            .await
            .map_err(|e| ToolCallError::internal(format!("Event poll failed: {e}")))?;

        if !events.is_empty() {
            last_event_time = tokio::time::Instant::now();
            total_events += events.len() as u64;

            for event in &events {
                // Track moderator's text output for final extraction
                let event_json = serde_json::to_value(event).unwrap_or_default();
                if let Some(payload) = event_json.get("payload") {
                    let is_moderator = event_json
                        .pointer("/meerkat_id")
                        .or_else(|| event_json.pointer("/member/meerkat_id"))
                        .and_then(Value::as_str)
                        .is_some_and(|id| id == "moderator");

                    if is_moderator {
                        if let Some(text) = payload.get("content").and_then(Value::as_str) {
                            if text.contains("## Panel Review Summary")
                                || text.contains("## Consensus")
                                || text.contains("## Recommendation")
                                || text.len() > 200
                            {
                                last_moderator_text = text.to_string();
                            }
                        }
                    }
                }

                event_cursor = event_json
                    .get("cursor")
                    .and_then(Value::as_u64)
                    .map(|c| c + 1)
                    .unwrap_or(event_cursor);
            }

            if let Some(token) = progress_token {
                send_progress(token, 0, 1, &format!("{total_events} messages exchanged")).await;
            }
        }

        // Check quiescence
        let quiet_duration = tokio::time::Instant::now() - last_event_time;
        if quiet_duration > std::time::Duration::from_millis(QUIET_THRESHOLD_MS)
            && total_events > 0
        {
            tracing::info!(total_events, "panel reached quiescence");
            break;
        }
    }

    if let Some(token) = progress_token {
        send_progress(token, 1, 1, "panel complete").await;
    }

    if last_moderator_text.is_empty() {
        // Fallback: collect any text from moderator events
        last_moderator_text = "Panel discussion completed but moderator summary not captured. Check agent logs for details.".to_string();
    }

    Ok(json!({"content": [{"type": "text", "text": last_moderator_text}]}))
}

// ── Progress notifications ──────────────────────────────────────────────────

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
    let mut stdout = tokio::io::stdout();
    let msg = format!("{notification}\n");
    let _ = stdout.write_all(msg.as_bytes()).await;
    let _ = stdout.flush().await;
}
