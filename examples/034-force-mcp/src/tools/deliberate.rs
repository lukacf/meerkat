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
    use meerkat_core::event::AgentEvent;

    if let Some(token) = progress_token {
        send_progress(token, 0, 1, "panel deliberating").await;
    }

    // Subscribe to mob-wide agent events (all members' streams merged)
    let mut router_handle = state
        .mob_state
        .subscribe_mob_events(mob_id)
        .await
        .map_err(|e| ToolCallError::internal(format!("Event subscription failed: {e}")))?;

    // Kick off the discussion by sending the task to the moderator
    state
        .mob_state
        .mob_send_message(mob_id, MeerkatId::from("moderator"), task.to_string())
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to trigger moderator: {e}")))?;

    // Drain events until quiescence
    const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
    const QUIET_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(15);

    let deadline = tokio::time::Instant::now() + MAX_WAIT;
    let mut last_event_time = tokio::time::Instant::now();
    let mut last_moderator_text = String::new();
    let mut total_events = 0u64;

    loop {
        let timeout = QUIET_THRESHOLD.min(
            deadline
                .saturating_duration_since(tokio::time::Instant::now()),
        );

        match tokio::time::timeout(timeout, router_handle.event_rx.recv()).await {
            Ok(Some(attributed)) => {
                last_event_time = tokio::time::Instant::now();
                total_events += 1;

                // Extract moderator's text outputs
                let is_moderator = attributed.source.as_str() == "moderator";
                if is_moderator {
                    match &attributed.envelope.payload {
                        AgentEvent::RunCompleted { result, .. } => {
                            if result.len() > last_moderator_text.len() {
                                last_moderator_text = result.clone();
                            }
                        }
                        AgentEvent::TextComplete { content, .. } => {
                            if content.len() > last_moderator_text.len() {
                                last_moderator_text = content.clone();
                            }
                        }
                        _ => {}
                    }
                }

                if total_events % 10 == 0 {
                    if let Some(token) = progress_token {
                        send_progress(token, 0, 1, &format!("{total_events} events")).await;
                    }
                }
            }
            Ok(None) => {
                // Channel closed — all agents done
                tracing::info!(total_events, "event channel closed");
                break;
            }
            Err(_) => {
                // Timeout — check quiescence or deadline
                let quiet = tokio::time::Instant::now() - last_event_time;
                if quiet >= QUIET_THRESHOLD && total_events > 0 {
                    tracing::info!(total_events, "panel reached quiescence");
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    tracing::warn!(total_events, "panel hit max wait time");
                    break;
                }
            }
        }
    }

    if let Some(token) = progress_token {
        send_progress(token, 1, 1, "panel complete").await;
    }

    if last_moderator_text.is_empty() {
        last_moderator_text =
            "Panel discussion completed but moderator summary not captured.".to_string();
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
