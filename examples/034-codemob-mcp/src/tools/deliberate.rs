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
    provider_params: Option<Value>,
}

pub async fn handle(
    state: &ForceState,
    arguments: &Value,
    progress_token: Option<Value>,
) -> Result<Value, ToolCallError> {
    let input: DeliberateInput = serde_json::from_value(arguments.clone())
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;

    let context = input.context.as_deref().unwrap_or("");
    let overrides = input.model_overrides.unwrap_or_default();

    // Borrow registry, extract what we need, then drop the borrow
    let (total_steps, has_flows, definition) = {
        let registry = state.pack_registry();
        let pack = registry.get(&input.pack).ok_or_else(|| {
            ToolCallError::invalid_params(format!(
                "Unknown pack: '{}'. Available: {}",
                input.pack,
                registry.list_names().join(", ")
            ))
        })?;
        let total_steps = pack.flow_step_count();
        let has_flows = total_steps > 0;
        let definition =
            pack.definition(&input.task, context, &overrides, input.provider_params.as_ref());
        (total_steps, has_flows, definition)
    };
    let mob_id = definition.id.clone();

    // Collect profile names and orchestrator before moving definition
    let profile_names: Vec<String> = definition.profiles.keys().map(|p| p.to_string()).collect();
    let orchestrator_name = definition
        .orchestrator
        .as_ref()
        .map(|o| o.profile.to_string())
        .unwrap_or_else(|| "moderator".to_string());

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

    // Fail fast if any agent failed to spawn — every flow step targets a
    // specific role, so a missing agent means a guaranteed downstream failure.
    let mut failed = Vec::new();
    for (i, result) in spawn_results.iter().enumerate() {
        match result {
            Ok(_) => tracing::info!(profile = %profile_names[i], "spawned"),
            Err(e) => failed.push(format!("{}: {e}", profile_names[i])),
        }
    }
    if !failed.is_empty() {
        // Clean up the partially-created mob before returning
        let _ = state.mob_state.mob_destroy(&mob_id).await;
        return Err(ToolCallError::internal(format!(
            "Spawn failed for: {}",
            failed.join("; ")
        )));
    }

    // Wait for all spawned agents to appear in the roster before running
    // the flow. The spawn command is processed by the mob actor, but the
    // roster update may not be visible yet when we query list_members().
    let expected = profile_names.len();
    let mut visible = 0;
    for _ in 0..20 {
        // 20 attempts × 50ms = 1s max wait
        match state.mob_state.mob_list_members(&mob_id).await {
            Ok(members) => {
                visible = members.len();
                if visible >= expected {
                    break;
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if visible < expected {
        let _ = state.mob_state.mob_destroy(&mob_id).await;
        return Err(ToolCallError::internal(format!(
            "Roster not ready: expected {expected} agents, only {visible} visible after 1s"
        )));
    }

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
        run_comms(state, &mob_id, &orchestrator_name, &prompt, progress_token.as_ref()).await
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

        // Count unique completed step IDs (not ledger entries, which can
        // have multiple entries per step for retries and per-target dispatch).
        let completed = run
            .step_ledger
            .iter()
            .filter(|e| e.status == StepRunStatus::Completed)
            .map(|e| &e.step_id)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let in_progress: Vec<_> = run
            .step_ledger
            .iter()
            .filter(|e| e.status == StepRunStatus::Dispatched)
            .map(|e| e.step_id.to_string())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
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
    orchestrator: &str,
    task: &str,
    progress_token: Option<&Value>,
) -> Result<Value, ToolCallError> {
    use meerkat_core::event::AgentEvent;

    if let Some(token) = progress_token {
        send_progress(token, 0, 1, "agents deliberating").await;
    }

    // Subscribe to mob-wide agent events (all members' streams merged)
    let mut router_handle = state
        .mob_state
        .subscribe_mob_events(mob_id)
        .await
        .map_err(|e| ToolCallError::internal(format!("Event subscription failed: {e}")))?;

    // Kick off the discussion by sending the task to the orchestrator
    state
        .mob_state
        .mob_send_message(mob_id, MeerkatId::from(orchestrator), task.to_string())
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to trigger {orchestrator}: {e}")))?;

    // Drain events until all agents are idle.
    //
    // Track active turns via RunStarted/RunCompleted. When active_turns
    // drops to 0 AND at least one turn has completed, the mob is done.
    // A generous idle grace period handles the gap between one agent
    // finishing and another being triggered by a comms message.
    const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(3600);
    const IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

    let deadline = tokio::time::Instant::now() + MAX_WAIT;
    let mut last_orchestrator_text = String::new();
    let mut total_events = 0u64;
    let mut active_turns: i64 = 0;
    let mut any_turn_completed = false;
    let mut idle_since: Option<tokio::time::Instant> = None;

    loop {
        let poll_timeout = if active_turns <= 0 && any_turn_completed {
            // All agents idle — wait grace period for a new turn to start
            let remaining_grace = idle_since
                .map(|t| IDLE_GRACE.saturating_sub(t.elapsed()))
                .unwrap_or(IDLE_GRACE);
            remaining_grace.min(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
        } else {
            // Agents are working — just enforce the hard deadline
            deadline.saturating_duration_since(tokio::time::Instant::now())
        };

        if poll_timeout.is_zero() {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(total_events, active_turns, "hit max wait time");
            } else {
                tracing::info!(total_events, "all agents idle, grace period expired");
            }
            break;
        }

        match tokio::time::timeout(poll_timeout, router_handle.event_rx.recv()).await {
            Ok(Some(attributed)) => {
                total_events += 1;

                match &attributed.envelope.payload {
                    AgentEvent::RunStarted { .. } => {
                        active_turns += 1;
                        idle_since = None;
                    }
                    AgentEvent::RunCompleted { .. } => {
                        active_turns -= 1;
                        any_turn_completed = true;
                        if active_turns <= 0 {
                            idle_since = Some(tokio::time::Instant::now());
                        }
                    }
                    _ => {}
                }

                // Capture the orchestrator's latest text output
                if attributed.source.as_str() == orchestrator {
                    match &attributed.envelope.payload {
                        AgentEvent::RunCompleted { result, .. } if !result.is_empty() => {
                            last_orchestrator_text = result.clone();
                        }
                        AgentEvent::TextComplete { content, .. } if !content.is_empty() => {
                            last_orchestrator_text = content.clone();
                        }
                        _ => {}
                    }
                }

                if total_events % 20 == 0 {
                    if let Some(token) = progress_token {
                        send_progress(
                            token, 0, 1,
                            &format!("{total_events} events, {active_turns} active"),
                        ).await;
                    }
                }
            }
            Ok(None) => {
                tracing::info!(total_events, "event channel closed");
                break;
            }
            Err(_) => {
                // Timeout expired — either grace period or deadline
                if active_turns <= 0 && any_turn_completed {
                    tracing::info!(total_events, "all agents idle, grace period expired");
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    tracing::warn!(total_events, active_turns, "hit max wait time");
                    break;
                }
            }
        }
    }

    if let Some(token) = progress_token {
        send_progress(token, 1, 1, "complete").await;
    }

    if last_orchestrator_text.is_empty() {
        last_orchestrator_text =
            format!("Discussion completed but {orchestrator} output not captured.");
    }

    Ok(json!({"content": [{"type": "text", "text": last_orchestrator_text}]}))
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
