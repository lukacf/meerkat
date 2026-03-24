use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use meerkat_mob::definition::FlowSpec;
use meerkat_mob::ids::{FlowId, MeerkatId, MobId};
use meerkat_mob::{MobDefinition, MobRun, MobRunStatus, SpawnMemberSpec, StepRunStatus};

use crate::state::ForceState;
use super::ToolCallError;

const DEFAULT_FLOW_STEP_TIMEOUT_MS: u64 = 30_000;
const FLOW_POLL_INTERVAL: Duration = Duration::from_millis(500);
const FLOW_WATCHDOG_BASE_SLACK_MS: u64 = 30_000;
const FLOW_WATCHDOG_PER_STEP_SLACK_MS: u64 = 5_000;

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
    let flow_timeout = if has_flows {
        Some(derive_flow_watchdog_timeout(&definition, &FlowId::from("main"))?)
    } else {
        None
    };

    // Create mob
    state
        .mob_state
        .mob_create_definition(definition)
        .await
        .map_err(|e| ToolCallError::internal(format!("Mob creation failed: {e}")))?;

    // Spawn one agent per profile (meerkat_id = profile name for simplicity)
    let specs: Vec<SpawnMemberSpec> = profile_names
        .iter()
        .map(|name| SpawnMemberSpec::new(name.as_str(), name.as_str()))
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
        run_flow(
            state,
            &mob_id,
            total_steps,
            progress_token.as_ref(),
            flow_timeout.expect("flow timeout computed when has_flows"),
        )
        .await
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
    flow_timeout: Duration,
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

    let last_completed = Arc::new(AtomicUsize::new(0));
    let progress_token = progress_token.cloned();

    let run = match poll_flow_until_terminal(
        flow_timeout,
        || async {
            state
                .mob_state
                .mob_flow_status(mob_id, run_id.clone())
                .await
                .map_err(|e| ToolCallError::internal(format!("Flow status failed: {e}")))
        },
        {
            let last_completed = Arc::clone(&last_completed);
            move |run| {
                let last_completed = Arc::clone(&last_completed);
                let progress_token = progress_token.clone();
                async move {
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

                    let previous = last_completed.load(Ordering::Relaxed);
                    if completed > previous {
                        last_completed.store(completed, Ordering::Relaxed);
                    }

                    if completed > previous || !in_progress.is_empty() {
                        if let Some(token) = progress_token.as_ref() {
                            let label = if !in_progress.is_empty() {
                                in_progress.join(", ")
                            } else {
                                "waiting".into()
                            };
                            send_progress(token, completed, total_steps, &label).await;
                        }
                    }

                    Ok(())
                }
            }
        },
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            let _ = state.mob_state.mob_cancel_flow(mob_id, run_id.clone()).await;
            return Err(error);
        }
    };

    match run.status() {
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

            Ok(json!({"content": [{"type": "text", "text": text}]}))
        }
        MobRunStatus::Failed => {
            let errors: Vec<String> = run
                .failure_ledger
                .iter()
                .map(|f| format!("{}: {}", f.step_id, f.reason))
                .collect();
            Err(ToolCallError::internal(format!(
                "Flow failed: {}",
                if errors.is_empty() { "unknown error".into() } else { errors.join("; ") }
            )))
        }
        MobRunStatus::Canceled => {
            Err(ToolCallError::internal("Flow was canceled".to_string()))
        }
        _ => Err(ToolCallError::internal(format!(
            "Flow reached non-terminal status '{}'",
            format_run_status(run.status())
        ))),
    }
}

fn derive_flow_watchdog_timeout(
    definition: &MobDefinition,
    flow_id: &FlowId,
) -> Result<Duration, ToolCallError> {
    let flow = definition.flows.get(flow_id).ok_or_else(|| {
        ToolCallError::internal(format!("Flow '{flow_id}' missing from deliberate pack"))
    })?;
    Ok(derive_flow_watchdog_timeout_from_spec(
        flow,
        definition.limits.as_ref().and_then(|limits| limits.max_flow_duration_ms),
        definition
            .limits
            .as_ref()
            .and_then(|limits| limits.max_step_retries)
            .unwrap_or(0),
    ))
}

fn derive_flow_watchdog_timeout_from_spec(
    flow: &FlowSpec,
    max_flow_duration_ms: Option<u64>,
    max_step_retries: u32,
) -> Duration {
    if let Some(limit_ms) = max_flow_duration_ms {
        return Duration::from_millis(limit_ms.saturating_add(FLOW_WATCHDOG_BASE_SLACK_MS));
    }

    let attempts_per_step = u64::from(max_step_retries).saturating_add(1);
    let total_step_budget_ms = flow.steps.values().fold(0u64, |acc, step| {
        let step_timeout_ms = step.timeout_ms.unwrap_or(DEFAULT_FLOW_STEP_TIMEOUT_MS);
        acc.saturating_add(step_timeout_ms.saturating_mul(attempts_per_step))
    });
    let per_step_slack_ms =
        FLOW_WATCHDOG_PER_STEP_SLACK_MS.saturating_mul(flow.steps.len() as u64);

    Duration::from_millis(
        total_step_budget_ms
            .saturating_add(FLOW_WATCHDOG_BASE_SLACK_MS)
            .saturating_add(per_step_slack_ms)
            .max(DEFAULT_FLOW_STEP_TIMEOUT_MS),
    )
}

async fn poll_flow_until_terminal<Fetch, FetchFut, Observe, ObserveFut>(
    flow_timeout: Duration,
    mut fetch_status: Fetch,
    mut observe: Observe,
) -> Result<MobRun, ToolCallError>
where
    Fetch: FnMut() -> FetchFut,
    FetchFut: Future<Output = Result<Option<MobRun>, ToolCallError>>,
    Observe: FnMut(MobRun) -> ObserveFut,
    ObserveFut: Future<Output = Result<(), ToolCallError>>,
{
    let started = tokio::time::Instant::now();
    let deadline = started + flow_timeout;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(flow_watchdog_timeout_error(flow_timeout));
        }

        tokio::time::sleep(FLOW_POLL_INTERVAL.min(deadline.saturating_duration_since(now))).await;

        let Some(run) = fetch_status().await? else {
            if tokio::time::Instant::now() >= deadline {
                return Err(flow_watchdog_timeout_error(flow_timeout));
            }
            continue;
        };

        observe(run.clone()).await?;

        if run.status().is_terminal() {
            return Ok(run);
        }
    }
}

fn flow_watchdog_timeout_error(timeout: Duration) -> ToolCallError {
    ToolCallError::internal(format!(
        "Flow did not reach a terminal state within {}s",
        timeout.as_secs()
    ))
}

fn format_run_status(status: &MobRunStatus) -> &'static str {
    match status {
        MobRunStatus::Pending => "pending",
        MobRunStatus::Running => "running",
        MobRunStatus::Completed => "completed",
        MobRunStatus::Failed => "failed",
        MobRunStatus::Canceled => "canceled",
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
        .mob_member_send(
            mob_id,
            MeerkatId::from(orchestrator),
            task.to_string().into(),
            meerkat_core::types::HandlingMode::Queue,
            None,
        )
        .await
        .map_err(|e| ToolCallError::internal(format!("Failed to trigger {orchestrator}: {e}")))?;

    // Drain events until all agents are idle.
    //
    // Track active turns via RunStarted/RunCompleted. When active_turns
    // drops to 0 AND at least one turn has completed, the mob is done.
    // A generous idle grace period handles the gap between one agent
    // finishing and another being triggered by a comms message.
    // Hard ceiling. Comms-based mobs can run iterative review loops where each
    // agent turn involves full LLM calls, tool use, or even code execution —
    // individual turns can take minutes. 1 hour accommodates long pipelines.
    const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(3600);
    // After all agents go idle, wait this long for a new turn to start (covers
    // the gap between one agent finishing and a comms message triggering another).
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

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use meerkat_core::types::ContentInput;
    use meerkat_mob::definition::{
        CollectionPolicy, DependencyMode, DispatchMode, FlowStepSpec, StepOutputFormat,
    };
    use meerkat_mob::ids::{ProfileName, StepId};

    fn sample_flow_spec(timeout_ms: u64) -> FlowSpec {
        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("one"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: ContentInput::from("step one"),
                depends_on: Vec::new(),
                dispatch_mode: DispatchMode::default(),
                collection_policy: CollectionPolicy::default(),
                condition: None,
                timeout_ms: Some(timeout_ms),
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: DependencyMode::default(),
                allowed_tools: None,
                blocked_tools: None,
                output_format: StepOutputFormat::Text,
            },
        );
        FlowSpec {
            description: Some("sample".into()),
            steps,
        }
    }

    #[test]
    fn derive_flow_watchdog_timeout_uses_step_budget_and_slack() {
        let flow = sample_flow_spec(1_000);
        let timeout = derive_flow_watchdog_timeout_from_spec(&flow, None, 1);
        assert_eq!(
            timeout,
            Duration::from_millis(
                1_000 * 2 + FLOW_WATCHDOG_BASE_SLACK_MS + FLOW_WATCHDOG_PER_STEP_SLACK_MS
            )
        );
    }

    #[tokio::test]
    async fn poll_flow_until_terminal_times_out_when_status_never_materializes() {
        let error = poll_flow_until_terminal(
            Duration::from_millis(25),
            || async { Ok(None) },
            |_run| async { Ok(()) },
        )
        .await
        .expect_err("missing flow status should time out instead of hanging forever");

        assert!(
            error.message.contains("did not reach a terminal state"),
            "unexpected error: {}",
            error.message
        );
    }
}
