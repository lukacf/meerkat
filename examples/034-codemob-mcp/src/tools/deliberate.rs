use meerkat::surface::{request_action, CancelActionInstallOutcome, RequestContext};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use meerkat_mob::definition::FlowSpec;
use meerkat_mob::ids::{FlowId, MobId};
use meerkat_mob::{
    mob_machine_run_public_result_class, mob_machine_run_status_is_terminal, MobDefinition,
    MobFlowRunPublicResultClass, MobRun, MobRunStatus, SpawnMemberSpec, StepRunStatus,
};

use super::{ProgressNotifier, ToolCallError};
use crate::state::ForceState;

const DEFAULT_FLOW_STEP_TIMEOUT_MS: u64 = 300_000; // 5 min: matches Opus call timeout
const FLOW_POLL_INTERVAL: Duration = Duration::from_millis(500);
const FLOW_WATCHDOG_BASE_SLACK_MS: u64 = 60_000; // 1 min base slack
const FLOW_WATCHDOG_PER_STEP_SLACK_MS: u64 = 30_000; // 30s per step slack

#[derive(Deserialize)]
struct DeliberateInput {
    pack: String,
    task: String,
    context: Option<String>,
    model_overrides: Option<BTreeMap<String, String>>,
    provider_params: Option<Value>,
    /// Continue an existing mob session instead of creating a new one.
    session_id: Option<String>,
}

pub async fn handle(
    state: &ForceState,
    arguments: &Value,
    progress_token: Option<Value>,
    progress_notifier: Option<ProgressNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    let input: DeliberateInput = serde_json::from_value(arguments.clone())
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;

    let context = input.context.as_deref().unwrap_or("");
    let overrides = input.model_overrides.unwrap_or_default();
    let provider_params = input
        .provider_params
        .as_ref()
        .map(|value| serde_json::from_value::<meerkat_core::ProviderParamsOverride>(value.clone()))
        .transpose()
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid provider_params: {e}")))?;

    // Borrow registry, extract what we need, then drop the borrow
    let (total_steps, definition) = {
        let registry = state.pack_registry();
        let pack = registry.get(&input.pack).ok_or_else(|| {
            ToolCallError::invalid_params(format!(
                "Unknown pack: '{}'. Available: {}",
                input.pack,
                registry.list_names().join(", ")
            ))
        })?;
        let total_steps = pack.flow_step_count();
        if total_steps == 0 {
            return Err(ToolCallError::invalid_params(format!(
                "Pack '{}' has no machine-owned flow; deliberate requires a 'main' flow so completion is resolved by MobMachine",
                input.pack
            )));
        }
        let definition = pack.definition(&overrides, provider_params.as_ref());
        (total_steps, definition)
    };

    // If session_id is provided, attempt to reuse an existing mob.
    let resuming = input.session_id.is_some();
    let mob_id = if let Some(ref sid) = input.session_id {
        MobId::from(sid.as_str())
    } else {
        definition.id.clone()
    };

    // Check if the mob already exists (resume path).
    let mob_exists = if resuming {
        state.mob_state.mob_status(&mob_id).await.is_ok()
    } else {
        false
    };

    let _setup_cancel = super::cancellation::cancellation_signal(request_context.as_ref()).await?;
    if let Some(context) = request_context.as_ref() {
        if !mob_exists {
            let mob_state_cleanup = state.mob_state.clone();
            let mob_id_for_cleanup = mob_id.clone();
            context.set_unpublished_cleanup(request_action(move || {
                let mob_state = mob_state_cleanup.clone();
                let mob_id = mob_id_for_cleanup.clone();
                async move {
                    let _ = mob_state.mob_destroy(&mob_id).await;
                }
            }));
        }
    }

    // Collect profile names before moving definition.
    let profile_names: Vec<String> = definition.profiles.keys().map(|p| p.to_string()).collect();
    let flow_timeout = derive_flow_watchdog_timeout(&definition, &FlowId::from("main"))?;

    if !mob_exists {
        check_cancelled(request_context.as_ref())?;
        // Override the mob id when resuming with a new mob
        let mut definition = definition;
        if resuming {
            definition.id = mob_id.clone();
        }

        // Create mob
        state
            .mob_state
            .mob_create_definition(definition)
            .await
            .map_err(|e| ToolCallError::internal(format!("Mob creation failed: {e}")))?;
        check_setup_cancellation(state, &mob_id, request_context.as_ref()).await?;

        // Spawn one agent per profile (agent_identity = profile name for simplicity)
        let specs: Vec<SpawnMemberSpec> = profile_names
            .iter()
            .map(|name| SpawnMemberSpec::new(name.as_str(), name.as_str()))
            .collect();

        let spawn_results = state
            .mob_state
            .mob_spawn_many(&mob_id, specs)
            .await
            .map_err(|e| ToolCallError::internal(format!("Spawn failed: {e}")))?;
        check_setup_cancellation(state, &mob_id, request_context.as_ref()).await?;

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
            check_setup_cancellation(state, &mob_id, request_context.as_ref()).await?;
            // 20 attempts × 50ms = 1s max wait
            if let Ok(members) = state.mob_state.mob_list_members(&mob_id).await {
                visible = members.len();
                if visible >= expected {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if visible < expected {
            let _ = state.mob_state.mob_destroy(&mob_id).await;
            return Err(ToolCallError::internal(format!(
                "Roster not ready: expected {expected} agents, only {visible} visible after 1s"
            )));
        }

        // Wait for autonomous kickoff turns to settle before the machine-owned
        // flow starts dispatching steps to the same agents.
        if let Some(token) = progress_token.as_ref() {
            send_progress(
                progress_notifier.as_ref(),
                token,
                0,
                1,
                &format!("waiting for {expected} agents to complete kickoff"),
            );
        }
        if let Err(e) = state
            .mob_state
            .mob_wait_kickoff(
                &mob_id,
                None,
                Some(120_000), // 2 min timeout — kickoff turns are simple LLM calls
            )
            .await
        {
            tracing::warn!(mob_id = %mob_id, error = %e, "kickoff barrier failed; proceeding anyway");
        }
        check_setup_cancellation(state, &mob_id, request_context.as_ref()).await?;
    }

    let result = run_flow(
        state,
        &mob_id,
        FlowProgress {
            total_steps,
            token: progress_token.as_ref(),
            notifier: progress_notifier.as_ref(),
        },
        request_context.as_ref(),
        flow_timeout,
        crate::packs::flow_params(&input.task, context),
    )
    .await;

    // A first call is ephemeral. The experimental reuse path retains a mob
    // after a caller has explicitly supplied its id.
    if !resuming {
        if let Err(e) = state.mob_state.mob_destroy(&mob_id).await {
            tracing::warn!(mob_id = %mob_id, error = %e, "mob cleanup failed");
        }
    }

    // Preserve the existing wire label for compatibility. This is a mob id,
    // not a reliable multi-call conversation handle in this example.
    match result {
        Ok(mut val) => {
            if let Some(arr) = val.get_mut("content").and_then(|c| c.as_array_mut()) {
                arr.push(json!({"type": "text", "text": format!("\n\n---\nsession_id: {mob_id}")}));
            }
            Ok(val)
        }
        err => err,
    }
}

// ── Flow-based execution (structured packs) ─────────────────────────────────

struct FlowProgress<'a> {
    total_steps: usize,
    token: Option<&'a Value>,
    notifier: Option<&'a ProgressNotifier>,
}

async fn run_flow(
    state: &ForceState,
    mob_id: &MobId,
    progress: FlowProgress<'_>,
    request_context: Option<&RequestContext>,
    flow_timeout: Duration,
    params: Value,
) -> Result<Value, ToolCallError> {
    let FlowProgress {
        total_steps,
        token: progress_token,
        notifier: progress_notifier,
    } = progress;
    check_cancelled(request_context)?;
    let flow_id = FlowId::from("main");

    if let Some(token) = progress_token {
        send_progress(progress_notifier, token, 0, total_steps, "starting flow");
    }

    let run_id = state
        .mob_state
        .mob_run_flow(mob_id, flow_id, params)
        .await
        .map_err(|e| ToolCallError::internal(format!("Flow start failed: {e}")))?;

    if let Some(context) = request_context {
        let mob_state = state.mob_state.clone();
        let mob_id_for_cancel = mob_id.clone();
        let run_id_for_cancel = run_id.clone();
        if context
            .install_cancel_action_or_cancelled(request_action(move || {
                let mob_state = mob_state.clone();
                let mob_id = mob_id_for_cancel.clone();
                let run_id = run_id_for_cancel.clone();
                async move {
                    let _ = mob_state.mob_cancel_flow(&mob_id, run_id.clone()).await;
                    let _ = mob_state.mob_destroy(&mob_id).await;
                }
            }))
            .await
            == CancelActionInstallOutcome::AlreadyCancelled
        {
            return Err(ToolCallError::cancelled());
        }
    }

    let last_completed = Arc::new(AtomicUsize::new(0));
    let last_progress = Arc::new(Mutex::new(None::<(usize, String)>));
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
            let last_progress = Arc::clone(&last_progress);
            move |run| {
                let last_completed = Arc::clone(&last_completed);
                let last_progress = Arc::clone(&last_progress);
                let progress_token = progress_token.clone();
                async move {
                    let (completed, in_progress) = flow_progress(&run)?;

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
                            let should_send = {
                                let mut last =
                                    last_progress.lock().expect("flow progress lock poisoned");
                                let current = (completed, label.clone());
                                if last.as_ref() == Some(&current) {
                                    false
                                } else {
                                    *last = Some(current);
                                    true
                                }
                            };
                            if should_send {
                                send_progress(
                                    progress_notifier,
                                    token,
                                    completed,
                                    total_steps,
                                    &label,
                                );
                            }
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
            let _ = state
                .mob_state
                .mob_cancel_flow(mob_id, run_id.clone())
                .await;
            return Err(error);
        }
    };

    let result_class = mob_machine_run_public_result_class(&run.run_id, run.status())
        .map_err(|error| ToolCallError::internal(error.to_string()))?;
    match result_class {
        MobFlowRunPublicResultClass::Success => {
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
        MobFlowRunPublicResultClass::Error => match run.status() {
            MobRunStatus::Failed => {
                let errors: Vec<String> = run
                    .failure_ledger
                    .iter()
                    .map(|f| format!("{}: {}", f.step_id, f.reason))
                    .collect();
                Err(ToolCallError::internal(format!(
                    "Flow failed: {}",
                    if errors.is_empty() {
                        "unknown error".into()
                    } else {
                        errors.join("; ")
                    }
                )))
            }
            MobRunStatus::Canceled => Err(ToolCallError::internal("Flow was canceled".to_string())),
            status => Err(ToolCallError::internal(format!(
                "Flow generated error result for status '{}'",
                format_run_status(status)
            ))),
        },
    }
}

fn check_cancelled(context: Option<&RequestContext>) -> Result<(), ToolCallError> {
    if context.is_some_and(RequestContext::cancel_already_requested) {
        Err(ToolCallError::cancelled())
    } else {
        Ok(())
    }
}

async fn check_setup_cancellation(
    state: &ForceState,
    mob_id: &MobId,
    context: Option<&RequestContext>,
) -> Result<(), ToolCallError> {
    if let Err(cancelled) = check_cancelled(context) {
        state.mob_state.mob_destroy(mob_id).await.map_err(|error| {
            ToolCallError::internal(format!("Mob cancellation cleanup failed: {error}"))
        })?;
        return Err(cancelled);
    }
    Ok(())
}

fn flow_progress(run: &MobRun) -> Result<(usize, Vec<String>), ToolCallError> {
    use meerkat_mob::run::flow_frame::{FlowNodeKind, FrameScope, NodeRunStatus};
    if !run.frames.is_empty() {
        let frame = run
            .frames
            .values()
            .find(|frame| frame.kernel_state.frame_scope == FrameScope::Root)
            .ok_or_else(|| ToolCallError::internal("Flow progress is missing its root frame"))?;
        let steps = frame.kernel_state.node_status.iter().filter(|(node, _)| {
            frame.kernel_state.node_kind.get(*node) == Some(&FlowNodeKind::Step)
        });
        let completed = steps
            .clone()
            .filter(|(_, status)| **status == NodeRunStatus::Completed)
            .count();
        let active = steps
            .filter(|(_, status)| **status == NodeRunStatus::Running)
            .map(|(node, _)| node.to_string())
            .collect();
        return Ok((completed, active));
    }
    let statuses = run
        .step_status_snapshot()
        .map_err(|error| ToolCallError::internal(error.to_string()))?;
    let completed = statuses
        .values()
        .filter(|status| **status == StepRunStatus::Completed)
        .count();
    let active = statuses
        .iter()
        .filter(|(_, status)| **status == StepRunStatus::Dispatched)
        .map(|(step, _)| step.to_string())
        .collect();
    Ok((completed, active))
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
        definition
            .limits
            .as_ref()
            .and_then(|limits| limits.max_flow_duration_ms),
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
    let per_step_slack_ms = FLOW_WATCHDOG_PER_STEP_SLACK_MS.saturating_mul(flow.steps.len() as u64);

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

        let terminal = mob_machine_run_status_is_terminal(&run.run_id, run.status())
            .map_err(|error| ToolCallError::internal(error.to_string()))?;
        if terminal {
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

// ── Progress notifications ──────────────────────────────────────────────────

fn send_progress(
    notifier: Option<&ProgressNotifier>,
    token: &Value,
    progress: usize,
    total: usize,
    label: &str,
) {
    if let Some(notifier) = notifier {
        notifier(token.clone(), progress, total, label.to_string());
    }
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
                output_format: Some(StepOutputFormat::Text),
                failure_policy: Default::default(),
            },
        );
        FlowSpec::new(Some("sample".into()), steps, None)
    }

    #[test]
    fn progress_uses_current_aggregate_not_per_target_history() {
        let ids = ["plan", "z_active", "a_active"];
        let mut flow_state = MobRun::flow_state_for_steps(ids.map(StepId::from)).unwrap();
        for (step, status) in [
            ("plan", "Completed"),
            ("z_active", "Dispatched"),
            ("a_active", "Dispatched"),
        ] {
            flow_state.step_status.insert(
                StepId::from(step),
                Some(serde_json::from_value(json!(status)).unwrap()),
            );
        }
        flow_state
            .step_target_counts
            .insert(StepId::from("z_active"), 2);
        flow_state
            .step_target_success_counts
            .insert(StepId::from("z_active"), 1);
        let mut run: MobRun = serde_json::from_value(json!({
            "run_id": uuid::Uuid::new_v4().to_string(),
            "mob_id":"synthetic", "flow_id":"main", "status":"running",
            "flow_state": flow_state, "activation_params": {},
            "created_at":"2026-01-01T00:00:00Z", "completed_at":null,
            "step_ledger":[
                {"step_id":"plan","agent_identity":"worker","status":"dispatched","output":null,"timestamp":"2026-01-01T00:00:00Z"},
                {"step_id":"plan","agent_identity":"worker","status":"completed","output":"done","timestamp":"2026-01-01T00:00:01Z"},
                {"step_id":"z_active","agent_identity":"first","status":"completed","output":"one target","timestamp":"2026-01-01T00:00:02Z"},
                {"step_id":"a_active","agent_identity":"retry","status":"failed","output":null,"timestamp":"2026-01-01T00:00:02Z"}
            ],
            "failure_ledger":[]
        })).unwrap();
        for _ in 0..20 {
            assert_eq!(
                flow_progress(&run).unwrap(),
                (1, vec!["a_active".into(), "z_active".into()])
            );
        }
        for status in run.flow_state.step_status.values_mut() {
            *status = Some(serde_json::from_value(json!("Completed")).unwrap());
        }
        assert_eq!(flow_progress(&run).unwrap(), (3, vec![]));

        use meerkat_mob::ids::{FlowNodeId, FrameId};
        use meerkat_mob::run::flow_frame::{FlowNodeKind, FrameScope, NodeRunStatus, State};
        let mut frame = State {
            frame_scope: FrameScope::Root,
            ..Default::default()
        };
        for (node, status) in [
            ("plan", NodeRunStatus::Completed),
            ("z_active", NodeRunStatus::Running),
            ("a_active", NodeRunStatus::Running),
        ] {
            frame
                .node_kind
                .insert(FlowNodeId::from(node), FlowNodeKind::Step);
            frame.node_status.insert(FlowNodeId::from(node), status);
        }
        run.frames.insert(
            FrameId::from("root"),
            meerkat_mob::run::FrameSnapshot {
                kernel_state: frame,
            },
        );
        assert_eq!(
            flow_progress(&run).unwrap(),
            (1, vec!["a_active".into(), "z_active".into()])
        );
        for status in run
            .frames
            .get_mut(&FrameId::from("root"))
            .unwrap()
            .kernel_state
            .node_status
            .values_mut()
        {
            *status = NodeRunStatus::Completed;
        }
        assert_eq!(flow_progress(&run).unwrap(), (3, vec![]));
    }

    #[tokio::test]
    async fn cancelled_setup_destroys_created_mob_before_member_admission() {
        use meerkat::surface::{noop_request_action, SurfaceRequestExecutor};
        let dir = tempfile::Builder::new()
            .prefix(".audit-setup-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let client = Arc::new(crate::tests::CaptureClient::default());
        let state = ForceState::with_test_client(dir.path(), client.clone());
        let definition = state
            .pack_registry()
            .get("advisor")
            .unwrap()
            .definition(&BTreeMap::new(), None);
        let mob_id = definition.id.clone();
        let executor = SurfaceRequestExecutor::new(Duration::from_secs(1));
        let context = executor.begin_request("setup", noop_request_action());
        let _signal = super::super::cancellation::cancellation_signal(Some(&context))
            .await
            .unwrap();
        state
            .mob_state
            .mob_create_definition(definition)
            .await
            .unwrap();
        executor.cancel_request("setup").await;
        assert_eq!(
            check_setup_cancellation(&state, &mob_id, Some(&context))
                .await
                .unwrap_err()
                .code,
            -32005
        );
        assert!(state.mob_state.mob_list().await.unwrap().is_empty());
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn derive_flow_watchdog_timeout_clamps_small_flows_to_default_step_timeout() {
        let flow = sample_flow_spec(1_000);
        let timeout = derive_flow_watchdog_timeout_from_spec(&flow, None, 1);
        assert_eq!(timeout, Duration::from_millis(DEFAULT_FLOW_STEP_TIMEOUT_MS));
    }

    #[test]
    fn derive_flow_watchdog_timeout_uses_step_budget_and_slack() {
        let flow = sample_flow_spec(300_000);
        let timeout = derive_flow_watchdog_timeout_from_spec(&flow, None, 1);
        assert_eq!(
            timeout,
            Duration::from_millis(
                300_000 * 2 // step budget * attempts
                    + FLOW_WATCHDOG_BASE_SLACK_MS
                    + FLOW_WATCHDOG_PER_STEP_SLACK_MS
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
