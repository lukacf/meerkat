use super::conditions::evaluate_condition;
use super::events::MobEventEmitter;
use super::flow_system_member_id;
use super::handle::MobHandle;
use super::path::resolve_context_path;
use super::supervisor::Supervisor;
use super::terminalization::{
    FlowTerminalizationAuthority, TerminalizationOutcome, TerminalizationTarget,
};
use super::topology::{MobTopologyService, PolicyDecision};
use super::turn_executor::{FlowTurnExecutor, FlowTurnOutcome, TimeoutDisposition};
use crate::definition::{
    CollectionPolicy, DispatchMode, FlowNodeSpec, FlowStepSpec, FrameSpec, FrameStepSpec,
    PolicyMode, StepOutputFormat,
};
use crate::error::MobError;
use crate::ids::{AgentIdentity, FlowId, FlowNodeId, FrameId, MeerkatId, RunId, StepId};
use crate::machines::mob_machine as mob_dsl;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, MobMachineFlowAuthorityToken,
    MobMachineFlowRunCommand, MobRun, MobRunStatus, StepLedgerEntry, StepRunStatus,
    apply_mob_machine_flow_run_command,
};
use crate::runtime::flow_frame_engine::{FlowFrameTerminalPhase, FrameStepProjection};
use crate::store::{MobEventStore, MobRunStore};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::ContentInput;
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct FlowEngine {
    executor: Arc<dyn FlowTurnExecutor>,
    handle: MobHandle,
    run_store: Arc<dyn MobRunStore>,
    emitter: MobEventEmitter,
    terminalization: FlowTerminalizationAuthority,
    topology: Arc<MobTopologyService>,
}

#[derive(Debug, Clone)]
pub struct FrameStepProjectionEffects {
    pub step_status: StepRunStatus,
    pub persist_output: bool,
    pub append_failure_ledger: bool,
    pub escalate_supervisor: bool,
}

#[derive(Debug, Clone)]
struct FrameStepProjectionOutcome {
    pub step_status: StepRunStatus,
    pub effects: Option<FrameStepProjectionEffects>,
}

#[derive(Debug, Clone)]
pub struct FrameStepProjectionRequest {
    pub frame_id: FrameId,
    pub node_id: FlowNodeId,
    pub append_failure_ledger: bool,
}

impl FrameStepProjectionRequest {
    pub fn completed(frame_id: FrameId, node_id: FlowNodeId) -> Self {
        Self {
            frame_id,
            node_id,
            append_failure_ledger: false,
        }
    }

    pub fn skipped(frame_id: FrameId, node_id: FlowNodeId) -> Self {
        Self {
            frame_id,
            node_id,
            append_failure_ledger: false,
        }
    }

    pub fn failed(frame_id: FrameId, node_id: FlowNodeId, append_failure_ledger: bool) -> Self {
        Self {
            frame_id,
            node_id,
            append_failure_ledger,
        }
    }
}

impl FlowEngine {
    pub fn new(
        executor: Arc<dyn FlowTurnExecutor>,
        handle: MobHandle,
        run_store: Arc<dyn MobRunStore>,
        event_store: Arc<dyn MobEventStore>,
        topology: Arc<MobTopologyService>,
    ) -> Self {
        let emitter = MobEventEmitter::new(event_store.clone(), handle.mob_id().clone());
        let terminalization = FlowTerminalizationAuthority::new(
            run_store.clone(),
            event_store,
            handle.mob_id().clone(),
        );
        Self {
            executor,
            handle,
            run_store,
            emitter,
            terminalization,
            topology,
        }
    }

    pub async fn execute_flow(
        &self,
        run_id: RunId,
        config: FlowRunConfig,
        activation_params: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<(), MobError> {
        let transitioned = self.start_run_state(&run_id).await?;
        if !transitioned && !self.is_run_running(&run_id).await? {
            return Ok(());
        }
        self.emitter
            .flow_started(
                run_id.clone(),
                config.flow_id.clone(),
                activation_params.clone(),
            )
            .await?;

        let flow_started_at = Instant::now();
        let flow_deadline = config
            .limits
            .as_ref()
            .and_then(|limits| limits.max_flow_duration_ms)
            .map(Duration::from_millis)
            .map(|limit| flow_started_at + limit);

        let synthesized_root;
        let root_spec = if let Some(root_spec) = &config.flow_spec.root {
            root_spec
        } else {
            synthesized_root = legacy_flat_steps_as_root_frame(&config.flow_spec.steps);
            &synthesized_root
        };
        // All flow execution is routed through FlowFrameEngine so branch,
        // dependency, loop, and ready-queue decisions remain MobMachine-owned.
        {
            let frame_id = crate::ids::FrameId::from(format!("{run_id}-root").as_str());
            let context = FlowContext {
                run_id: run_id.clone(),
                activation_params: activation_params.clone(),
                step_outputs: IndexMap::new(),
                loop_outputs: IndexMap::new(),
            };
            let adapter = FlowTurnExecutorAdapter::new(
                self.clone(),
                config.clone(),
                cancel.clone(),
                flow_deadline,
            );
            let max_depth = config
                .limits
                .as_ref()
                .and_then(|l| l.max_frame_depth)
                .unwrap_or(0) as u32;
            let max_active_frames = config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_frames)
                .unwrap_or(0) as u32;
            let frame_engine = super::flow_frame_engine::FlowFrameEngine::new(
                self.run_store.clone(),
                Arc::new(adapter),
                self.handle.clone(),
                max_depth,
                max_active_frames,
            );
            let frame_result = frame_engine
                .execute_frame(&run_id, &frame_id, root_spec, &context)
                .await;

            match frame_result {
                Ok(frame_outcome) => {
                    let root_phase = root_frame_terminal_phase_from_machine(
                        &self.handle.query_machine_state().await?,
                        &frame_id,
                    )?;
                    match root_phase {
                        FlowFrameTerminalPhase::Failed => {
                            let machine_state = self.handle.query_machine_state().await?;
                            for projection_record in frame_outcome.step_projections.values() {
                                let step_id = &projection_record.step_id;
                                let step_status = frame_step_projection_status_from_machine(
                                    &machine_state,
                                    &run_id,
                                    step_id,
                                    &projection_record.frame_id,
                                    &projection_record.node_id,
                                )?;
                                if !matches!(
                                    step_status,
                                    StepRunStatus::Failed | StepRunStatus::Skipped
                                ) {
                                    continue;
                                }
                                let failure = frame_outcome.step_failures.get(step_id);
                                let request = match (&step_status, failure) {
                                    (StepRunStatus::Failed, Some(failure)) => {
                                        FrameStepProjectionRequest::failed(
                                            projection_record.frame_id.clone(),
                                            projection_record.node_id.clone(),
                                            failure.append_failure_ledger,
                                        )
                                    }
                                    (StepRunStatus::Failed, None) => {
                                        FrameStepProjectionRequest::failed(
                                            projection_record.frame_id.clone(),
                                            projection_record.node_id.clone(),
                                            true,
                                        )
                                    }
                                    (StepRunStatus::Skipped, _) => {
                                        FrameStepProjectionRequest::skipped(
                                            projection_record.frame_id.clone(),
                                            projection_record.node_id.clone(),
                                        )
                                    }
                                    _ => unreachable!("filtered terminal status above"),
                                };
                                let projection = self
                                    .project_frame_step_status(&run_id, step_id, request)
                                    .await?;
                                let reason = match (&projection.step_status, failure) {
                                    (StepRunStatus::Failed, Some(failure)) => {
                                        failure.reason.as_str()
                                    }
                                    (StepRunStatus::Failed, None) => {
                                        "frame execution marked step failed"
                                    }
                                    (StepRunStatus::Skipped, _) => {
                                        "auto-skipped by failed frame execution"
                                    }
                                    _ => {
                                        return Err(MobError::Internal(format!(
                                            "failed frame projected non-failed step status {:?} \
                                             for step '{step_id}' in run '{run_id}'",
                                            projection.step_status
                                        )));
                                    }
                                };
                                if self
                                    .apply_frame_step_projection(
                                        projection.effects,
                                        &run_id,
                                        step_id,
                                        None,
                                        Some(reason.to_owned()),
                                    )
                                    .await?
                                {
                                    let supervisor =
                                        Supervisor::new(self.handle.clone(), self.emitter.clone());
                                    supervisor
                                        .escalate(&config, &run_id, step_id, reason)
                                        .await?;
                                    supervisor.force_reset().await?;
                                }
                            }
                            return self
                                .fail_run(
                                    &run_id,
                                    &config.flow_id,
                                    MobError::FlowFailed {
                                        run_id: run_id.clone(),
                                        reason: "root frame sealed failed".into(),
                                    },
                                )
                                .await;
                        }
                        FlowFrameTerminalPhase::Canceled => {
                            self.cancel_unfinished_steps(&run_id).await?;
                            let _ = self
                                .terminalize_canceled(run_id.clone(), config.flow_id.clone())
                                .await?;
                            return Ok(());
                        }
                        FlowFrameTerminalPhase::Completed => {}
                    }

                    // Project the typed frame outcome into MobMachine-owned run state once at the
                    // run/frame seam. This avoids reconstructing run truth from output
                    // side maps.
                    if let TerminalizationOutcome::Transitioned = self
                        .terminalize_completed_from_frame(
                            &run_id,
                            config.flow_id.clone(),
                            &frame_outcome.outputs,
                            &frame_outcome.step_projections,
                        )
                        .await?
                    {
                        tracing::debug!(run_id = %run_id, "frame-based flow completed terminalization applied");
                    }

                    Ok(())
                }
                Err(e) => {
                    // Preserve the cancel/fail distinction (dogma Rule 4: one semantic
                    // condition, one canonical terminal path). The flat-step path calls
                    // terminalize_canceled when canceled — the frame path must too.
                    if matches!(e, MobError::RunCanceled(_)) {
                        self.cancel_unfinished_steps(&run_id).await?;
                        if let TerminalizationOutcome::Transitioned = self
                            .terminalize_canceled(run_id.clone(), config.flow_id.clone())
                            .await?
                        {
                            tracing::debug!(run_id = %run_id, "frame-based flow canceled terminalization applied");
                        }
                        return Ok(());
                    }
                    return self.fail_run(&run_id, &config.flow_id, e).await;
                }
            }
        }
    }

    /// Canonical step execution: condition -> topology -> targets -> dispatch ->
    /// retry -> schema -> ledger -> events -> supervisor -> FanOut aggregation.
    ///
    /// Does NOT touch run or frame machine state. It may emit
    /// step-local target events while work is running, but terminal step/run
    /// projection belongs to the caller-owned machine seam. Returns
    /// `StepGuardOutcome::Completed(output)` on success or
    /// `StepGuardOutcome::Skipped { reason }` when the step's condition
    /// evaluates to false.
    ///
    /// This is the SINGLE implementation of step execution. Both the flat-step
    /// inner loop and the frame-based FlowTurnExecutorAdapter delegate here.
    pub(crate) async fn execute_step_with_all_guards(
        &self,
        request: StepExecutionRequest<'_>,
        control: StepExecutionControl<'_>,
    ) -> Result<StepGuardOutcome, MobError> {
        let StepExecutionRequest {
            run_id,
            step_id,
            step,
            context,
            config,
        } = request;
        let StepExecutionControl {
            evaluate_condition_guard,
            cancel,
            flow_deadline,
        } = control;

        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(MobError::RunCanceled(run_id.clone()));
        }
        if let Some(deadline) = flow_deadline
            && Instant::now() >= deadline
        {
            return Err(MobError::FlowFailed {
                run_id: run_id.clone(),
                reason: flow_deadline_reason(deadline, run_id),
            });
        }

        // ── 1. Condition check ─────────────────────────────────────────────
        if evaluate_condition_guard
            && step
                .condition
                .as_ref()
                .is_some_and(|cond| !evaluate_condition(cond, context))
        {
            let reason = "condition evaluated to false".to_string();
            return Ok(StepGuardOutcome::Skipped { reason });
        }

        // ── 2. Prompt rendering ────────────────────────────────────────────
        let prompt = match render_content_input_template(&step.message, context) {
            Ok(prompt) => prompt,
            Err(error) => {
                return Ok(StepGuardOutcome::Failed {
                    reason: format!("template render failed for step '{step_id}': {error}"),
                    failure_ledger_recorded: false,
                });
            }
        };

        // ── 3. Topology check ──────────────────────────────────────────────
        if let (Some(topology_spec), Some(from_role)) =
            (&config.topology, &config.orchestrator_role)
            && matches!(
                self.topology.evaluate(from_role, &step.role),
                PolicyDecision::Deny
            )
        {
            if matches!(topology_spec.mode, PolicyMode::Strict) {
                return Err(MobError::TopologyViolation {
                    from_role: from_role.clone(),
                    to_role: step.role.clone(),
                });
            }
            self.emitter
                .topology_violation(from_role.clone(), step.role.clone())
                .await?;
        }

        // ── 4. Target selection ────────────────────────────────────────────
        let mut targets: Vec<MeerkatId> = self
            .handle
            .list_runnable_members()
            .await
            .into_iter()
            .filter(|entry| entry.role == step.role)
            .map(|entry| entry.agent_identity)
            .collect();
        targets.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        let available_targets = targets.len();
        if let CollectionPolicy::Quorum { n } = step.collection_policy
            && available_targets < n as usize
        {
            let error = MobError::InsufficientTargets {
                step_id: step_id.clone(),
                required: n,
                available: available_targets,
            };
            return Err(error);
        }
        let required_targets = match step.collection_policy {
            CollectionPolicy::Quorum { n } => n as usize,
            CollectionPolicy::All | CollectionPolicy::Any => 1usize,
        };
        if available_targets < required_targets {
            let error = MobError::InsufficientTargets {
                step_id: step_id.clone(),
                required: required_targets.min(u8::MAX as usize) as u8,
                available: available_targets,
            };
            return Ok(StepGuardOutcome::Failed {
                reason: error.to_string(),
                failure_ledger_recorded: false,
            });
        }

        // ── 5. OneToOne: truncate to 1 target ──────────────────────────────
        if targets.len() > 1 && matches!(step.dispatch_mode, DispatchMode::OneToOne) {
            targets.truncate(1);
        }

        // ── 6. Multi-target (FanOut/FanIn) ─────────────────────────────────
        if targets.len() > 1 {
            return self
                .dispatch_multi_target_canonical(
                    run_id,
                    step_id,
                    &targets,
                    step,
                    context,
                    config,
                    cancel,
                    flow_deadline,
                )
                .await;
        }

        // ── 7. Single-target dispatch with retry loop ──────────────────────
        let target = targets.into_iter().next().ok_or_else(|| {
            MobError::Internal(format!(
                "no target available for role '{}' in step '{step_id}'",
                step.role
            ))
        })?;

        let requested_step_timeout = Duration::from_millis(step.timeout_ms.unwrap_or(30_000));
        let (step_timeout, flow_deadline_timeout_reason) =
            effective_step_timeout(requested_step_timeout, flow_deadline, run_id)?;
        let flow_tool_overlay = step_tool_overlay(step);

        let max_retries = config
            .limits
            .as_ref()
            .and_then(|l| l.max_step_retries)
            .unwrap_or(0) as usize;

        match self
            .execute_single_target_with_retries(
                run_id,
                step_id,
                &target,
                &prompt,
                flow_tool_overlay,
                step.output_format.clone(),
                step_timeout,
                max_retries,
                cancel,
                flow_deadline_timeout_reason,
            )
            .await?
        {
            Ok(value) => {
                // Wrap the single-target value through aggregate_output so the
                // final shape matches multi-target dispatch (e.g. FanOut + All
                // wraps as {"target_id": value}).
                let aggregate = aggregate_output(
                    &step.dispatch_mode,
                    &step.collection_policy,
                    &[(target.clone(), value)],
                );

                // ── 8. Schema validation (on aggregate output) ────────────
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Err(schema_err) =
                        validate_schema_ref(schema_ref, step_id, &aggregate).await
                {
                    let reason = schema_err.to_string();
                    return Ok(StepGuardOutcome::Failed {
                        reason,
                        failure_ledger_recorded: false,
                    });
                }

                self.emitter
                    .step_completed(run_id.clone(), step_id.clone())
                    .await?;
                Ok(StepGuardOutcome::Completed(aggregate))
            }
            Err(failure) => Ok(StepGuardOutcome::Failed {
                reason: failure.reason,
                failure_ledger_recorded: failure.failure_ledger_recorded,
            }),
        }
    }

    /// Multi-target dispatch for FanOut/FanIn, used by `execute_step_with_all_guards`.
    /// Parallel dispatch: all targets are dispatched concurrently via `FuturesUnordered`,
    /// each with its own retry loop. Returns the aggregated output matching the step's
    /// collection policy.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_multi_target_canonical(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        targets: &[MeerkatId],
        step: &FlowStepSpec,
        context: &FlowContext,
        config: &FlowRunConfig,
        cancel: Option<&CancellationToken>,
        flow_deadline: Option<Instant>,
    ) -> Result<StepGuardOutcome, MobError> {
        let step_timeout = Duration::from_millis(step.timeout_ms.unwrap_or(30_000));
        let flow_tool_overlay = step_tool_overlay(step);
        let (step_timeout, flow_deadline_timeout_reason) =
            effective_step_timeout(step_timeout, flow_deadline, run_id)?;
        let prompt = match render_content_input_template(&step.message, context) {
            Ok(prompt) => prompt,
            Err(error) => {
                return Ok(StepGuardOutcome::Failed {
                    reason: format!("template render failed for step '{step_id}': {error}"),
                    failure_ledger_recorded: false,
                });
            }
        };
        let max_retries = config
            .limits
            .as_ref()
            .and_then(|l| l.max_step_retries)
            .unwrap_or(0) as usize;

        let mut in_flight = FuturesUnordered::new();
        for target in targets {
            let engine = self.clone();
            let run_id = run_id.clone();
            let step_id = step_id.clone();
            let target = target.clone();
            let prompt = prompt.clone();
            let overlay = flow_tool_overlay.clone();
            let output_format = step.output_format.clone();
            let flow_deadline_timeout_reason = flow_deadline_timeout_reason.clone();
            in_flight.push(async move {
                let result = engine
                    .execute_single_target_with_retries(
                        &run_id,
                        &step_id,
                        &target,
                        &prompt,
                        overlay,
                        output_format,
                        step_timeout,
                        max_retries,
                        cancel,
                        flow_deadline_timeout_reason,
                    )
                    .await;
                (target, result)
            });
        }

        let mut successes: Vec<(MeerkatId, Value)> = Vec::new();
        let mut failures: Vec<StepTargetFailure> = Vec::new();

        while let Some((target, result)) = in_flight.next().await {
            match result {
                Err(e) => return Err(e),
                Ok(Ok(value)) => {
                    successes.push((target, value));
                }
                Ok(Err(failure)) => {
                    failures.push(failure);
                }
            }
        }

        // Enforce collection policy (mirrors flat-step path's collection_satisfied check).
        let policy_satisfied = match &step.collection_policy {
            CollectionPolicy::All => failures.is_empty(),
            CollectionPolicy::Any => !successes.is_empty(),
            CollectionPolicy::Quorum { n } => successes.len() >= *n as usize,
        };

        if !policy_satisfied {
            let combined = if failures.is_empty() {
                format!(
                    "collection policy {:?} not satisfied: {} successes, {} failures",
                    step.collection_policy,
                    successes.len(),
                    failures.len()
                )
            } else {
                failures
                    .iter()
                    .map(|failure| failure.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            return Ok(StepGuardOutcome::Failed {
                reason: combined,
                failure_ledger_recorded: !failures.is_empty()
                    && failures
                        .iter()
                        .all(|failure| failure.failure_ledger_recorded),
            });
        }

        // Aggregate output using the step's dispatch/collection policy.
        let aggregate = aggregate_output(&step.dispatch_mode, &step.collection_policy, &successes);

        // Validate aggregated schema (mirrors flat-step path at flow.rs:453-468).
        if let Some(schema_ref) = &step.expected_schema_ref
            && let Err(schema_err) = validate_schema_ref(schema_ref, step_id, &aggregate).await
        {
            let reason = schema_err.to_string();
            return Ok(StepGuardOutcome::Failed {
                reason,
                failure_ledger_recorded: false,
            });
        }

        self.emitter
            .step_completed(run_id.clone(), step_id.clone())
            .await?;
        Ok(StepGuardOutcome::Completed(aggregate))
    }

    /// Single-target dispatch with retry loop. Used by both the single-target path
    /// in `execute_step_with_all_guards` and the parallel multi-target path in
    /// `dispatch_multi_target_canonical`. Returns `Ok(Ok(value))` on success,
    /// `Ok(Err(failure))` on terminal target failure, and `Err(MobError)` on
    /// infrastructure errors that should abort the entire step.
    #[allow(clippy::too_many_arguments)]
    async fn execute_single_target_with_retries(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        prompt: &ContentInput,
        flow_tool_overlay: Option<TurnToolOverlay>,
        output_format: StepOutputFormat,
        step_timeout: Duration,
        max_retries: usize,
        cancel: Option<&CancellationToken>,
        flow_deadline_timeout_reason: Option<String>,
    ) -> Result<Result<Value, StepTargetFailure>, MobError> {
        let mut attempt = 0usize;
        loop {
            self.emitter
                .step_dispatched(run_id.clone(), step_id.clone(), target.clone())
                .await?;
            self.run_store
                .append_step_entry_if_absent(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_id.clone(),
                        agent_identity: AgentIdentity::from(target.as_str()),
                        status: StepRunStatus::Dispatched,
                        output: None,
                        timestamp: Utc::now(),
                    },
                )
                .await?;

            let ticket = match self
                .executor
                .dispatch(
                    run_id,
                    step_id,
                    target,
                    prompt.clone(),
                    flow_tool_overlay.clone(),
                )
                .await
            {
                Ok(ticket) => ticket,
                Err(error) => {
                    return Ok(Err(StepTargetFailure::unrecorded(error.to_string())));
                }
            };

            let terminal = match cancel {
                Some(cancel_token) => {
                    tokio::select! {
                        result = self.executor.await_terminal(ticket.clone(), step_timeout) => result,
                        () = cancel_token.cancelled() => {
                            match self.executor.on_timeout(ticket.clone()).await {
                                Ok(_) => return Err(MobError::RunCanceled(run_id.clone())),
                                Err(e) => {
                                    return Err(MobError::FlowFailed {
                                        run_id: run_id.clone(),
                                        reason: e.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
                None => {
                    self.executor
                        .await_terminal(ticket.clone(), step_timeout)
                        .await
                }
            };

            match terminal {
                Ok(FlowTurnOutcome::Completed { output }) => {
                    match parse_output_value(&output, step_id, target, &output_format) {
                        Ok(value) => {
                            self.run_store
                                .append_step_entry(
                                    run_id,
                                    StepLedgerEntry {
                                        step_id: step_id.clone(),
                                        agent_identity: AgentIdentity::from(target.as_str()),
                                        status: StepRunStatus::Completed,
                                        output: Some(value.clone()),
                                        timestamp: Utc::now(),
                                    },
                                )
                                .await?;
                            self.emitter
                                .step_target_completed(
                                    run_id.clone(),
                                    step_id.clone(),
                                    target.clone(),
                                )
                                .await?;
                            return Ok(Ok(value));
                        }
                        Err(reason) => {
                            if attempt < max_retries {
                                attempt += 1;
                                self.emitter
                                    .step_target_failed(
                                        run_id.clone(),
                                        step_id.clone(),
                                        target.clone(),
                                        reason.clone(),
                                    )
                                    .await?;
                                continue;
                            }
                            self.run_store
                                .append_step_entry(
                                    run_id,
                                    StepLedgerEntry {
                                        step_id: step_id.clone(),
                                        agent_identity: AgentIdentity::from(target.as_str()),
                                        status: StepRunStatus::Failed,
                                        output: None,
                                        timestamp: Utc::now(),
                                    },
                                )
                                .await?;
                            self.emitter
                                .step_target_failed(
                                    run_id.clone(),
                                    step_id.clone(),
                                    target.clone(),
                                    reason.clone(),
                                )
                                .await?;
                            self.run_store
                                .append_failure_entry(
                                    run_id,
                                    FailureLedgerEntry {
                                        step_id: step_id.clone(),
                                        reason: reason.clone(),
                                        timestamp: Utc::now(),
                                    },
                                )
                                .await?;
                            return Ok(Err(StepTargetFailure::recorded(reason)));
                        }
                    }
                }
                Ok(FlowTurnOutcome::Failed { reason }) => {
                    if attempt < max_retries {
                        attempt += 1;
                        self.emitter
                            .step_target_failed(
                                run_id.clone(),
                                step_id.clone(),
                                target.clone(),
                                reason.clone(),
                            )
                            .await?;
                        continue;
                    }
                    self.run_store
                        .append_step_entry(
                            run_id,
                            StepLedgerEntry {
                                step_id: step_id.clone(),
                                agent_identity: AgentIdentity::from(target.as_str()),
                                status: StepRunStatus::Failed,
                                output: None,
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    self.emitter
                        .step_target_failed(
                            run_id.clone(),
                            step_id.clone(),
                            target.clone(),
                            reason.clone(),
                        )
                        .await?;
                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                step_id: step_id.clone(),
                                reason: reason.clone(),
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    return Ok(Err(StepTargetFailure::recorded(reason)));
                }
                Ok(FlowTurnOutcome::Canceled) => {
                    if cancel.is_some_and(CancellationToken::is_cancelled) {
                        return Err(MobError::RunCanceled(run_id.clone()));
                    }
                    let cancel_reason = "canceled".to_string();
                    self.run_store
                        .append_step_entry(
                            run_id,
                            StepLedgerEntry {
                                step_id: step_id.clone(),
                                agent_identity: AgentIdentity::from(target.as_str()),
                                status: StepRunStatus::Failed,
                                output: None,
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    self.emitter
                        .step_target_failed(
                            run_id.clone(),
                            step_id.clone(),
                            target.clone(),
                            cancel_reason.clone(),
                        )
                        .await?;
                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                step_id: step_id.clone(),
                                reason: cancel_reason.clone(),
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    return Ok(Err(StepTargetFailure::recorded(cancel_reason)));
                }
                Err(MobError::FlowTurnTimedOut) => {
                    let timeout_reason = match self.executor.on_timeout(ticket).await {
                        Ok(TimeoutDisposition::Detached) => {
                            format!("timeout after {}ms", step_timeout.as_millis())
                        }
                        Ok(TimeoutDisposition::Canceled) => {
                            format!("timeout + canceled after {}ms", step_timeout.as_millis())
                        }
                        Err(e) => {
                            return Err(MobError::FlowFailed {
                                run_id: run_id.clone(),
                                reason: e.to_string(),
                            });
                        }
                    };
                    if let Some(reason) = flow_deadline_timeout_reason.clone() {
                        return Err(MobError::FlowFailed {
                            run_id: run_id.clone(),
                            reason,
                        });
                    }
                    if attempt < max_retries {
                        attempt += 1;
                        self.emitter
                            .step_target_failed(
                                run_id.clone(),
                                step_id.clone(),
                                target.clone(),
                                timeout_reason,
                            )
                            .await?;
                        continue;
                    }
                    self.run_store
                        .append_step_entry(
                            run_id,
                            StepLedgerEntry {
                                step_id: step_id.clone(),
                                agent_identity: AgentIdentity::from(target.as_str()),
                                status: StepRunStatus::Failed,
                                output: None,
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    self.emitter
                        .step_target_failed(
                            run_id.clone(),
                            step_id.clone(),
                            target.clone(),
                            timeout_reason.clone(),
                        )
                        .await?;
                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                step_id: step_id.clone(),
                                reason: timeout_reason.clone(),
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    return Ok(Err(StepTargetFailure::recorded(timeout_reason)));
                }
                Err(other) => return Err(other),
            }
        }
    }

    /// Supervisor escalation guard used by `execute_step_with_all_guards`.
    async fn fail_run(
        &self,
        run_id: &RunId,
        flow_id: &FlowId,
        error: MobError,
    ) -> Result<(), MobError> {
        let reason = error.to_string();
        self.fail_unfinished_steps(run_id).await?;
        let _ = self
            .terminalize_failed(run_id.clone(), flow_id.clone(), reason)
            .await?;
        Ok(())
    }

    async fn apply_frame_step_projection(
        &self,
        effects: Option<FrameStepProjectionEffects>,
        run_id: &RunId,
        step_id: &StepId,
        output: Option<Value>,
        reason: Option<String>,
    ) -> Result<bool, MobError> {
        let Some(effects) = effects else {
            return Ok(false);
        };
        self.run_store
            .append_step_entry(
                run_id,
                StepLedgerEntry {
                    step_id: step_id.clone(),
                    agent_identity: AgentIdentity::from(flow_system_member_id().as_str()),
                    status: effects.step_status.clone(),
                    output: if effects.persist_output { output } else { None },
                    timestamp: Utc::now(),
                },
            )
            .await?;
        match effects.step_status {
            StepRunStatus::Completed => {}
            StepRunStatus::Skipped => {
                self.emitter
                    .step_skipped(
                        run_id.clone(),
                        step_id.clone(),
                        reason.unwrap_or_else(|| "auto-skipped by frame execution".into()),
                    )
                    .await?;
            }
            StepRunStatus::Failed => {
                let reason = reason.unwrap_or_else(|| "frame step failed".into());
                if effects.append_failure_ledger {
                    self.run_store
                        .append_failure_entry(
                            run_id,
                            FailureLedgerEntry {
                                step_id: step_id.clone(),
                                reason: reason.clone(),
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                }
                self.emitter
                    .step_failed(run_id.clone(), step_id.clone(), reason)
                    .await?;
            }
            StepRunStatus::Dispatched | StepRunStatus::Canceled => {
                return Err(MobError::Internal(format!(
                    "frame step projection cannot append non-terminal status {:?} for step '{step_id}'",
                    effects.step_status
                )));
            }
        }
        Ok(effects.escalate_supervisor)
    }

    async fn apply_skip_projection(
        &self,
        effects: Option<Vec<flow_run::Effect>>,
        run_id: &RunId,
        step_id: &StepId,
        reason: String,
    ) -> Result<(), MobError> {
        let Some(effects) = effects else {
            return Ok(());
        };
        if let Some(step_notice) = find_step_notice_effect(&effects, step_id)? {
            self.run_store
                .append_step_entry(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_notice.step_id.clone(),
                        agent_identity: AgentIdentity::from(flow_system_member_id().as_str()),
                        status: step_notice.status,
                        output: None,
                        timestamp: Utc::now(),
                    },
                )
                .await?;
            self.emitter
                .step_skipped(run_id.clone(), step_id.clone(), reason)
                .await?;
        }
        Ok(())
    }

    async fn apply_failure_projection(
        &self,
        effects: Option<Vec<flow_run::Effect>>,
        run_id: &RunId,
        step_id: &StepId,
        reason: String,
        append_failure_ledger: bool,
    ) -> Result<bool, MobError> {
        let Some(effects) = effects else {
            return Ok(false);
        };
        if let Some(step_notice) = find_step_notice_effect(&effects, step_id)? {
            self.run_store
                .append_step_entry(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_notice.step_id.clone(),
                        agent_identity: AgentIdentity::from(flow_system_member_id().as_str()),
                        status: step_notice.status,
                        output: None,
                        timestamp: Utc::now(),
                    },
                )
                .await?;
            self.emitter
                .step_failed(run_id.clone(), step_id.clone(), reason.clone())
                .await?;
        }
        if append_failure_ledger
            && has_effect(
                &effects,
                FlowRunEffectKind::AppendFailureLedger,
                Some(step_id),
                None,
            )
        {
            self.run_store
                .append_failure_entry(
                    run_id,
                    FailureLedgerEntry {
                        step_id: step_id.clone(),
                        reason,
                        timestamp: Utc::now(),
                    },
                )
                .await?;
        }
        Ok(has_effect(
            &effects,
            FlowRunEffectKind::EscalateSupervisor,
            Some(step_id),
            None,
        ))
    }

    async fn is_run_running(&self, run_id: &RunId) -> Result<bool, MobError> {
        Ok(self
            .run_store
            .get_run(run_id)
            .await?
            .is_some_and(|run| run.status == MobRunStatus::Running))
    }

    async fn run_snapshot(&self, run_id: &RunId) -> Result<MobRun, MobError> {
        self.run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))
    }

    async fn start_run_state(&self, run_id: &RunId) -> Result<bool, MobError> {
        let run = self.run_snapshot(run_id).await?;
        if run.status != MobRunStatus::Pending {
            return Ok(false);
        }
        let command = MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {});
        Ok(self
            .handle
            .commit_flow_run_command(run_id, command, "flow_start_run")
            .await?
            .is_some())
    }

    async fn ordered_steps(&self, run_id: &RunId) -> Result<Vec<StepId>, MobError> {
        self.run_snapshot(run_id).await?.ordered_steps()
    }

    async fn step_status(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<StepRunStatus>, MobError> {
        Ok(self
            .run_snapshot(run_id)
            .await?
            .step_status_snapshot()?
            .remove(step_id))
    }

    async fn cas_flow_input_with_effects(
        &self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.handle
            .commit_flow_run_command(run_id, command, "flow_run_command")
            .await
    }

    pub(super) async fn start_run_state_with_machine_state(
        &self,
        run_id: &RunId,
        machine_state: mob_dsl::MobMachineState,
        authority: MobMachineFlowAuthorityToken,
        context: &'static str,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        let command = MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {});
        for attempt in 0..5u32 {
            let run = self.run_snapshot(run_id).await?;
            if run.status != MobRunStatus::Pending {
                return Ok(None);
            }
            let outcome = apply_mob_machine_flow_run_command(
                &run.flow_state,
                &machine_state,
                run_id,
                command.clone(),
                authority,
            )?;
            let transitioned = self
                .run_store
                .cas_run_snapshot(
                    run_id,
                    MobRunStatus::Pending,
                    &run.flow_state,
                    MobRunStatus::Running,
                    &outcome.next_state,
                )
                .await?;
            if transitioned {
                return Ok(Some(outcome.effects));
            }
            if attempt < 4 {
                tracing::debug!(attempt, context, "flow start CAS contention, retrying");
            }
        }
        Err(MobError::Internal(format!(
            "{context}: CAS contention after 5 attempts for run {run_id}"
        )))
    }

    pub(super) async fn apply_command_with_machine_state(
        &self,
        run_id: &RunId,
        command: MobMachineFlowRunCommand,
        machine_state: mob_dsl::MobMachineState,
        authority: MobMachineFlowAuthorityToken,
        context: &'static str,
    ) -> Result<Vec<flow_run::Effect>, MobError> {
        for attempt in 0..5u32 {
            let run = self.run_snapshot(run_id).await?;
            let outcome = apply_mob_machine_flow_run_command(
                &run.flow_state,
                &machine_state,
                run_id,
                command.clone(),
                authority,
            )?;
            let transitioned = self
                .run_store
                .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
                .await?;
            if transitioned {
                return Ok(outcome.effects);
            }
            if attempt < 4 {
                tracing::debug!(attempt, context, "flow.rs CAS contention, retrying");
            }
        }
        Err(MobError::Internal(format!(
            "{context}: CAS contention after 5 attempts for run {run_id}"
        )))
    }

    async fn condition_passed(&self, run_id: &RunId, step_id: &StepId) -> Result<bool, MobError> {
        Ok(self
            .cas_flow_input_with_effects(
                run_id,
                MobMachineFlowRunCommand::ConditionPassed(flow_run::inputs::ConditionPassed {
                    step_id: step_id.clone(),
                }),
            )
            .await?
            .is_some())
    }

    async fn skip_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.cas_flow_input_with_effects(
            run_id,
            MobMachineFlowRunCommand::SkipStep(flow_run::inputs::SkipStep {
                step_id: step_id.clone(),
            }),
        )
        .await
    }

    async fn fail_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.cas_flow_input_with_effects(
            run_id,
            MobMachineFlowRunCommand::FailStep(flow_run::inputs::FailStep {
                step_id: step_id.clone(),
            }),
        )
        .await
    }

    pub(crate) async fn cancel_unfinished_steps(&self, run_id: &RunId) -> Result<(), MobError> {
        for step_id in self.ordered_steps(run_id).await? {
            if self
                .step_status(run_id, &step_id)
                .await?
                .is_some_and(|status| status.is_terminal())
            {
                continue;
            }
            let _ = self
                .cas_flow_input_with_effects(
                    run_id,
                    MobMachineFlowRunCommand::CancelStep(flow_run::inputs::CancelStep {
                        step_id: step_id.clone(),
                    }),
                )
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn fail_unfinished_steps(&self, run_id: &RunId) -> Result<(), MobError> {
        for step_id in self.ordered_steps(run_id).await? {
            if self
                .step_status(run_id, &step_id)
                .await?
                .is_some_and(|status| status.is_terminal())
            {
                continue;
            }
            let _ = self
                .cas_flow_input_with_effects(
                    run_id,
                    MobMachineFlowRunCommand::FailStep(flow_run::inputs::FailStep {
                        step_id: step_id.clone(),
                    }),
                )
                .await?;
        }
        Ok(())
    }

    async fn dispatch_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.cas_flow_input_with_effects(
            run_id,
            MobMachineFlowRunCommand::DispatchStep(flow_run::inputs::DispatchStep {
                step_id: step_id.clone(),
            }),
        )
        .await
    }

    async fn complete_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.cas_flow_input_with_effects(
            run_id,
            MobMachineFlowRunCommand::CompleteStep(flow_run::inputs::CompleteStep {
                step_id: step_id.clone(),
            }),
        )
        .await
    }

    async fn record_step_output_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.cas_flow_input_with_effects(
            run_id,
            MobMachineFlowRunCommand::RecordStepOutput(flow_run::inputs::RecordStepOutput {
                step_id: step_id.clone(),
            }),
        )
        .await
    }

    async fn condition_rejected_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<flow_run::Effect>>, MobError> {
        self.cas_flow_input_with_effects(
            run_id,
            MobMachineFlowRunCommand::ConditionRejected(flow_run::inputs::ConditionRejected {
                step_id: step_id.clone(),
            }),
        )
        .await
    }

    async fn project_frame_step_status(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        request: FrameStepProjectionRequest,
    ) -> Result<FrameStepProjectionOutcome, MobError> {
        let FrameStepProjectionRequest {
            frame_id,
            node_id,
            append_failure_ledger: requested_failure_ledger_append,
        } = request;
        let machine_state = self.handle.query_machine_state().await?;
        let step_status = frame_step_projection_status_from_machine(
            &machine_state,
            run_id,
            step_id,
            &frame_id,
            &node_id,
        )?;

        if let Some(projected_status) =
            machine_run_step_status_from_machine(&machine_state, run_id, step_id)?
            && projected_status.is_terminal()
        {
            if projected_status != step_status {
                return Err(MobError::Internal(format!(
                    "project_frame_step_status: MobMachine run projection has status \
                     {projected_status:?} for run '{run_id}' step '{step_id}', but frame \
                     '{frame_id}' node '{node_id}' projects {step_status:?}"
                )));
            }
            return Ok(FrameStepProjectionOutcome {
                step_status,
                effects: None,
            });
        }

        let effects = self
            .cas_flow_input_with_effects(
                run_id,
                MobMachineFlowRunCommand::ProjectFrameStepStatus(
                    flow_run::inputs::ProjectFrameStepStatus {
                        step_id: step_id.clone(),
                        frame_id,
                        node_id,
                        append_failure_ledger: requested_failure_ledger_append,
                    },
                ),
            )
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "project_frame_step_status: transition returned no effects for run '{run_id}' \
                     step '{step_id}'"
                ))
            })?;
        let projected_status = find_step_notice_effect(&effects, step_id)?
            .map(|notice| notice.status)
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "project_frame_step_status: transition emitted no step notice for run \
                     '{run_id}' step '{step_id}'"
                ))
            })?;

        Ok(FrameStepProjectionOutcome {
            step_status: projected_status.clone(),
            effects: Some(FrameStepProjectionEffects {
                step_status: projected_status.clone(),
                persist_output: matches!(projected_status, StepRunStatus::Completed),
                append_failure_ledger: has_effect(
                    &effects,
                    FlowRunEffectKind::AppendFailureLedger,
                    Some(step_id),
                    None,
                ),
                escalate_supervisor: has_effect(
                    &effects,
                    FlowRunEffectKind::EscalateSupervisor,
                    Some(step_id),
                    None,
                ),
            }),
        })
    }

    async fn terminalize_completed_from_frame(
        &self,
        run_id: &RunId,
        flow_id: FlowId,
        outputs: &IndexMap<StepId, Value>,
        step_projections: &IndexMap<StepId, FrameStepProjection>,
    ) -> Result<TerminalizationOutcome, MobError> {
        for step_id in self.ordered_steps(run_id).await? {
            let projection_record = step_projections.get(&step_id).ok_or_else(|| {
                MobError::Internal(format!(
                    "terminalize_completed_from_frame missing machine-owned frame projection \
                     for step '{step_id}' in run '{run_id}'"
                ))
            })?;
            let projection = self
                .project_frame_step_status(
                    run_id,
                    &step_id,
                    FrameStepProjectionRequest::completed(
                        projection_record.frame_id.clone(),
                        projection_record.node_id.clone(),
                    ),
                )
                .await?;
            match &projection.step_status {
                StepRunStatus::Completed => {
                    let _ = self
                        .apply_frame_step_projection(
                            projection.effects,
                            run_id,
                            &step_id,
                            outputs.get(&step_id).cloned(),
                            None,
                        )
                        .await?;
                }
                StepRunStatus::Skipped => {
                    let _ = self
                        .apply_frame_step_projection(
                            projection.effects,
                            run_id,
                            &step_id,
                            None,
                            Some("auto-skipped by frame execution".into()),
                        )
                        .await?;
                }
                other => {
                    return Err(MobError::Internal(format!(
                        "terminalize_completed_from_frame cannot project status {other:?} \
                         for step '{step_id}' in run '{run_id}'"
                    )));
                }
            }
        }

        self.terminalize_completed(run_id.clone(), flow_id).await
    }

    pub(crate) async fn terminalize_completed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalize(run_id, flow_id, TerminalizationTarget::Completed)
            .await
    }

    pub(crate) async fn terminalize_failed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalize(run_id, flow_id, TerminalizationTarget::Failed { reason })
            .await
    }

    pub(crate) async fn terminalize_canceled(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalize(run_id, flow_id, TerminalizationTarget::Canceled)
            .await
    }

    pub(super) async fn terminalize_canceled_with_machine_state(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        machine_state: mob_dsl::MobMachineState,
    ) -> Result<TerminalizationOutcome, MobError> {
        let input =
            MobMachineFlowRunCommand::TerminalizeCanceled(flow_run::inputs::TerminalizeCanceled {});
        let authority_input = input.authority_input(&run_id);
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        self.terminalize_with_machine_state(
            run_id,
            flow_id,
            TerminalizationTarget::Canceled,
            input,
            machine_state,
            authority,
        )
        .await
    }

    async fn terminalize(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
    ) -> Result<TerminalizationOutcome, MobError> {
        let input = match &target {
            TerminalizationTarget::Completed => MobMachineFlowRunCommand::TerminalizeCompleted(
                flow_run::inputs::TerminalizeCompleted {},
            ),
            TerminalizationTarget::Failed { .. } => {
                MobMachineFlowRunCommand::TerminalizeFailed(flow_run::inputs::TerminalizeFailed {})
            }
            TerminalizationTarget::Canceled => MobMachineFlowRunCommand::TerminalizeCanceled(
                flow_run::inputs::TerminalizeCanceled {},
            ),
        };
        self.handle
            .commit_flow_terminalization(run_id, flow_id, target, input, "flow_terminalize")
            .await
    }

    pub(super) async fn terminalize_with_machine_state(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
        input: MobMachineFlowRunCommand,
        machine_state: mob_dsl::MobMachineState,
        authority: MobMachineFlowAuthorityToken,
    ) -> Result<TerminalizationOutcome, MobError> {
        let next_status = target.status();
        self.verify_terminal_run_steps(&run_id).await?;
        for attempt in 0..5u32 {
            let run = self.run_snapshot(&run_id).await?;
            if run.status.is_terminal()
                && !matches!(
                    (&target, &run.status),
                    (TerminalizationTarget::Canceled, MobRunStatus::Failed)
                )
            {
                return Ok(TerminalizationOutcome::Noop);
            }
            let next_state = apply_mob_machine_flow_run_command(
                &run.flow_state,
                &machine_state,
                &run_id,
                input.clone(),
                authority,
            )?
            .next_state;
            let transitioned = self
                .run_store
                .cas_run_snapshot(
                    &run_id,
                    run.status.clone(),
                    &run.flow_state,
                    next_status.clone(),
                    &next_state,
                )
                .await?;
            if transitioned {
                return self
                    .terminalization
                    .record_persisted_terminalization(run_id, flow_id, target)
                    .await;
            }
            if attempt < 4 {
                tracing::debug!(attempt, "terminalize CAS contention, retrying");
            }
        }
        Err(MobError::Internal(format!(
            "CAS contention after 5 attempts for run {run_id}"
        )))
    }

    async fn verify_terminal_run_steps(&self, run_id: &RunId) -> Result<(), MobError> {
        let steps = self.ordered_steps(run_id).await?;
        for step_id in &steps {
            match self.step_status(run_id, step_id).await? {
                Some(status) if status.is_terminal() => {}
                Some(status) => {
                    return Err(MobError::Internal(format!(
                        "TerminalRunStepInvariant violated: run '{run_id}' is terminal but \
                         step '{step_id}' has non-terminal status {status:?}"
                    )));
                }
                None => {
                    return Err(MobError::Internal(format!(
                        "TerminalRunStepInvariant violated: run '{run_id}' is terminal but \
                         step '{step_id}' has no MobMachine-owned terminal status"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn legacy_flat_steps_as_root_frame(steps: &IndexMap<StepId, FlowStepSpec>) -> FrameSpec {
    let nodes = steps
        .iter()
        .map(|(step_id, step)| {
            (
                FlowNodeId::from(step_id.as_str()),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: step_id.clone(),
                    depends_on: step
                        .depends_on
                        .iter()
                        .map(|dependency| FlowNodeId::from(dependency.as_str()))
                        .collect(),
                    depends_on_mode: step.depends_on_mode.clone(),
                    branch: step.branch.clone(),
                }),
            )
        })
        .collect();
    FrameSpec { nodes }
}

/// Outcome of canonical step execution via `execute_step_with_all_guards`.
pub(crate) enum StepGuardOutcome {
    Completed(Value),
    Skipped {
        reason: String,
    },
    Failed {
        reason: String,
        failure_ledger_recorded: bool,
    },
}

#[derive(Debug)]
struct StepTargetFailure {
    reason: String,
    failure_ledger_recorded: bool,
}

impl StepTargetFailure {
    fn recorded(reason: String) -> Self {
        Self {
            reason,
            failure_ledger_recorded: true,
        }
    }

    fn unrecorded(reason: String) -> Self {
        Self {
            reason,
            failure_ledger_recorded: false,
        }
    }
}

pub(crate) struct StepExecutionRequest<'a> {
    run_id: &'a RunId,
    step_id: &'a StepId,
    step: &'a FlowStepSpec,
    context: &'a FlowContext,
    config: &'a FlowRunConfig,
}

pub(crate) struct StepExecutionControl<'a> {
    evaluate_condition_guard: bool,
    cancel: Option<&'a CancellationToken>,
    flow_deadline: Option<Instant>,
}

#[derive(Debug, Clone)]
struct StepNoticeEffect {
    step_id: StepId,
    status: StepRunStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowRunEffectKind {
    AdmitStepWork,
    EmitStepNotice,
    PersistStepOutput,
    AppendFailureLedger,
    EscalateSupervisor,
}

impl FlowRunEffectKind {
    fn parse(effect: &flow_run::Effect) -> Option<Self> {
        match effect {
            flow_run::Effect::AdmitStepWork(_) => Some(Self::AdmitStepWork),
            flow_run::Effect::EmitStepNotice(_) => Some(Self::EmitStepNotice),
            flow_run::Effect::PersistStepOutput(_) => Some(Self::PersistStepOutput),
            flow_run::Effect::AppendFailureLedger(_) => Some(Self::AppendFailureLedger),
            flow_run::Effect::EscalateSupervisor(_) => Some(Self::EscalateSupervisor),
            _ => None,
        }
    }
}

fn step_tool_overlay(step: &FlowStepSpec) -> Option<TurnToolOverlay> {
    if step.allowed_tools.is_none() && step.blocked_tools.is_none() {
        return None;
    }
    Some(TurnToolOverlay {
        allowed_tools: step.allowed_tools.clone(),
        blocked_tools: step.blocked_tools.clone(),
    })
}

fn find_step_notice_effect(
    effects: &[flow_run::Effect],
    expected_step_id: &StepId,
) -> Result<Option<StepNoticeEffect>, MobError> {
    for effect in effects {
        if FlowRunEffectKind::parse(effect) != Some(FlowRunEffectKind::EmitStepNotice) {
            continue;
        }
        let Some(step_id) = effect_step_id(effect)? else {
            continue;
        };
        if &step_id != expected_step_id {
            continue;
        }
        let status = effect_step_status(effect)?;
        return Ok(Some(StepNoticeEffect { step_id, status }));
    }
    Ok(None)
}

fn has_effect(
    effects: &[flow_run::Effect],
    expected: FlowRunEffectKind,
    expected_step_id: Option<&StepId>,
    expected_target_id: Option<&MeerkatId>,
) -> bool {
    effects.iter().any(|effect| {
        if FlowRunEffectKind::parse(effect) != Some(expected) {
            return false;
        }
        if let Some(step_id) = expected_step_id {
            match effect_step_id(effect) {
                Ok(Some(candidate)) if &candidate == step_id => {}
                _ => return false,
            }
        }
        if let Some(target_id) = expected_target_id {
            match effect_target_id(effect) {
                Ok(Some(candidate)) if &candidate == target_id => {}
                _ => return false,
            }
        }
        true
    })
}

fn effect_step_id(effect: &flow_run::Effect) -> Result<Option<StepId>, MobError> {
    let step_id = match effect {
        flow_run::Effect::EmitStepNotice(payload) => Some(payload.step_id.clone()),
        flow_run::Effect::PersistStepOutput(payload) => Some(payload.step_id.clone()),
        flow_run::Effect::AppendFailureLedger(payload) => Some(payload.step_id.clone()),
        flow_run::Effect::AdmitStepWork(payload) => Some(payload.step_id.clone()),
        flow_run::Effect::EscalateSupervisor(payload) => Some(payload.step_id.clone()),
        _ => None,
    };
    Ok(step_id)
}

fn effect_target_id(effect: &flow_run::Effect) -> Result<Option<MeerkatId>, MobError> {
    let target_id = match effect {
        flow_run::Effect::ProjectTargetSuccess(payload) => Some(payload.target_id.clone()),
        flow_run::Effect::ProjectTargetFailure(payload) => Some(payload.target_id.clone()),
        flow_run::Effect::ProjectTargetCanceled(payload) => Some(payload.target_id.clone()),
        _ => None,
    };
    Ok(target_id)
}

fn effect_step_status(effect: &flow_run::Effect) -> Result<StepRunStatus, MobError> {
    match effect {
        flow_run::Effect::EmitStepNotice(payload) => {
            Ok(step_status_from_flow_run(payload.step_status))
        }
        other => Err(MobError::Internal(format!(
            "flow_run effect {other:?} does not carry step_status"
        ))),
    }
}

fn step_status_from_flow_run(status: flow_run::StepRunStatus) -> StepRunStatus {
    match status {
        flow_run::StepRunStatus::Dispatched => StepRunStatus::Dispatched,
        flow_run::StepRunStatus::Completed => StepRunStatus::Completed,
        flow_run::StepRunStatus::Failed => StepRunStatus::Failed,
        flow_run::StepRunStatus::Skipped => StepRunStatus::Skipped,
        flow_run::StepRunStatus::Canceled => StepRunStatus::Canceled,
    }
}

fn parse_output_value(
    raw: &str,
    step_id: &StepId,
    target: &MeerkatId,
    format: &StepOutputFormat,
) -> Result<Value, String> {
    if matches!(format, StepOutputFormat::Text) {
        return Ok(Value::String(raw.to_string()));
    }

    // Try parsing the raw output as-is first.
    if let Ok(value) = serde_json::from_str(raw) {
        return Ok(value);
    }

    // LLMs sometimes wrap JSON in markdown code fences (```json ... ```).
    // Strip them and retry before reporting a parse error.
    let stripped = strip_code_fences(raw);
    serde_json::from_str(&stripped).map_err(|error| {
        let excerpt = if raw.chars().count() > 200 {
            format!("{}...", raw.chars().take(200).collect::<String>())
        } else {
            raw.to_string()
        };
        format!(
            "malformed JSON output for step '{step_id}' target '{target}': {error}; raw_output={excerpt:?}"
        )
    })
}

/// Strip markdown code fences from LLM output.
///
/// Handles ```` ```json ... ``` ````, ```` ``` ... ``` ````, and leading/trailing whitespace.
fn strip_code_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(open_idx) = trimmed.find("```") else {
        return trimmed.to_string();
    };

    let rest = &trimmed[open_idx + 3..];
    let after_tag = match rest.find('\n') {
        Some(i) => &rest[i + 1..],
        None => return trimmed.to_string(),
    };
    let body = after_tag.find("```").map_or(after_tag, |i| &after_tag[..i]);
    body.trim().to_string()
}

fn aggregate_output(
    dispatch_mode: &DispatchMode,
    policy: &CollectionPolicy,
    successes: &[(MeerkatId, Value)],
) -> Value {
    if matches!(dispatch_mode, DispatchMode::FanIn) {
        return Value::Array(
            successes
                .iter()
                .map(|(target, output)| {
                    serde_json::json!({
                        "target": target.as_str(),
                        "output": output
                    })
                })
                .collect(),
        );
    }

    if matches!(policy, CollectionPolicy::Any) {
        return successes
            .first()
            .map_or(Value::Null, |(_, output)| output.clone());
    }

    let mut output = Map::new();
    for (target, value) in successes {
        output.insert(target.as_str().to_string(), value.clone());
    }
    Value::Object(output)
}

async fn validate_schema_ref(
    schema_ref: &str,
    step_id: &StepId,
    output: &Value,
) -> Result<(), MobError> {
    let schema = match serde_json::from_str::<Value>(schema_ref) {
        Ok(inline) => inline,
        Err(_) => {
            #[cfg(not(target_arch = "wasm32"))]
            let raw = tokio::fs::read_to_string(schema_ref)
                .await
                .map_err(|error| MobError::SchemaValidation {
                    step_id: step_id.clone(),
                    message: format!("failed to read schema ref '{schema_ref}': {error}"),
                })?;
            #[cfg(target_arch = "wasm32")]
            return Err(MobError::SchemaValidation {
                step_id: step_id.clone(),
                message: format!("file-based schema ref '{schema_ref}' is not supported on wasm32"),
            });
            #[cfg(not(target_arch = "wasm32"))]
            {
                serde_json::from_str(&raw).map_err(|error| MobError::SchemaValidation {
                    step_id: step_id.clone(),
                    message: format!("invalid schema JSON at '{schema_ref}': {error}"),
                })?
            }
        }
    };
    let validator =
        jsonschema::validator_for(&schema).map_err(|error| MobError::SchemaValidation {
            step_id: step_id.clone(),
            message: format!("schema compile failed: {error}"),
        })?;

    if validator.is_valid(output) {
        return Ok(());
    }

    let message = validator
        .iter_errors(output)
        .next()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "schema validator reported invalid output for step '{step_id}' but yielded no errors"
            ))
        })?
        .to_string();

    Err(MobError::SchemaValidation {
        step_id: step_id.clone(),
        message,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplateSegment {
    Text(String),
    Placeholder(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateSyntaxKind {
    UnmatchedOpen,
    UnmatchedClose,
    EmptyPlaceholder,
    NestedPlaceholderOpen,
    LoneClosingBraceInPlaceholder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TemplateSyntaxError {
    kind: TemplateSyntaxKind,
    byte_offset: usize,
    line: usize,
    column: usize,
}

impl TemplateSyntaxError {
    fn new(template: &str, byte_offset: usize, kind: TemplateSyntaxKind) -> Self {
        let offset = byte_offset.min(template.len());
        let mut line = 1usize;
        let mut column = 1usize;
        for ch in template[..offset].chars() {
            if ch == '\n' {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(1);
            }
        }
        Self {
            kind,
            byte_offset: offset,
            line,
            column,
        }
    }

    fn kind_message(&self) -> &'static str {
        match self.kind {
            TemplateSyntaxKind::UnmatchedOpen => "unmatched '{{'",
            TemplateSyntaxKind::UnmatchedClose => "unmatched '}}'",
            TemplateSyntaxKind::EmptyPlaceholder => "empty placeholder",
            TemplateSyntaxKind::NestedPlaceholderOpen => "nested '{{' inside placeholder",
            TemplateSyntaxKind::LoneClosingBraceInPlaceholder => "single '}' inside placeholder",
        }
    }
}

impl std::fmt::Display for TemplateSyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid template syntax at line {}, column {} (byte {}): {}",
            self.line,
            self.column,
            self.byte_offset,
            self.kind_message()
        )
    }
}

fn parse_template(template: &str) -> Result<Vec<TemplateSegment>, TemplateSyntaxError> {
    let bytes = template.as_bytes();
    let mut cursor = 0usize;
    let mut text_start = 0usize;
    let mut segments = Vec::new();

    while cursor < bytes.len() {
        if bytes[cursor] == b'{' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'{' {
            if text_start < cursor {
                segments.push(TemplateSegment::Text(
                    template[text_start..cursor].to_string(),
                ));
            }

            let placeholder_start = cursor;
            cursor = cursor.saturating_add(2);
            let expression_start = cursor;
            let mut closed = false;

            while cursor < bytes.len() {
                if bytes[cursor] == b'{' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'{' {
                    return Err(TemplateSyntaxError::new(
                        template,
                        cursor,
                        TemplateSyntaxKind::NestedPlaceholderOpen,
                    ));
                }
                if bytes[cursor] == b'}' {
                    if cursor + 1 < bytes.len() && bytes[cursor + 1] == b'}' {
                        let expression = template[expression_start..cursor].trim();
                        if expression.is_empty() {
                            return Err(TemplateSyntaxError::new(
                                template,
                                expression_start,
                                TemplateSyntaxKind::EmptyPlaceholder,
                            ));
                        }
                        segments.push(TemplateSegment::Placeholder(expression.to_string()));
                        cursor = cursor.saturating_add(2);
                        text_start = cursor;
                        closed = true;
                        break;
                    }
                    return Err(TemplateSyntaxError::new(
                        template,
                        cursor,
                        TemplateSyntaxKind::LoneClosingBraceInPlaceholder,
                    ));
                }
                cursor = cursor.saturating_add(1);
            }

            if !closed {
                return Err(TemplateSyntaxError::new(
                    template,
                    placeholder_start,
                    TemplateSyntaxKind::UnmatchedOpen,
                ));
            }

            continue;
        }

        if bytes[cursor] == b'}' && cursor + 1 < bytes.len() && bytes[cursor + 1] == b'}' {
            return Err(TemplateSyntaxError::new(
                template,
                cursor,
                TemplateSyntaxKind::UnmatchedClose,
            ));
        }

        cursor = cursor.saturating_add(1);
    }

    if text_start < bytes.len() {
        segments.push(TemplateSegment::Text(template[text_start..].to_string()));
    }

    Ok(segments)
}

fn render_template(template: &str, context: &FlowContext) -> Result<String, String> {
    let segments = parse_template(template).map_err(|error| error.to_string())?;
    let mut rendered = String::new();

    for segment in segments {
        match segment {
            TemplateSegment::Text(text) => rendered.push_str(&text),
            TemplateSegment::Placeholder(expression) => {
                let replacement =
                    resolve_template_value(&expression, context).map_or(Value::Null, Clone::clone);
                match replacement {
                    Value::String(text) => rendered.push_str(&text),
                    other => rendered.push_str(&other.to_string()),
                }
            }
        }
    }

    Ok(rendered)
}

fn render_content_input_template(
    template: &ContentInput,
    context: &FlowContext,
) -> Result<ContentInput, String> {
    match template {
        ContentInput::Text(text) => render_template(text, context).map(ContentInput::Text),
        ContentInput::Blocks(blocks) => {
            let mut rendered = Vec::with_capacity(blocks.len());
            for block in blocks {
                match block {
                    meerkat_core::types::ContentBlock::Text { text } => {
                        rendered.push(meerkat_core::types::ContentBlock::Text {
                            text: render_template(text, context)?,
                        });
                    }
                    other => rendered.push(other.clone()),
                }
            }
            Ok(ContentInput::Blocks(rendered))
        }
    }
}

fn resolve_template_value<'a>(expression: &str, context: &'a FlowContext) -> Option<&'a Value> {
    resolve_context_path(context, expression)
}

// ─── FlowTurnExecutorAdapter ────────────────────────────────────────────────

/// Thin adapter that implements `FrameStepExecutor` by delegating to
/// `FlowEngine::execute_step_with_all_guards`. All step execution logic
/// (condition, topology, targets, dispatch, retry, schema, ledger, events,
/// supervisor, FanOut aggregation) lives in the canonical method on FlowEngine.
struct FlowTurnExecutorAdapter {
    engine: FlowEngine,
    config: FlowRunConfig,
    cancel: CancellationToken,
    flow_deadline: Option<Instant>,
}

impl FlowTurnExecutorAdapter {
    fn new(
        engine: FlowEngine,
        config: FlowRunConfig,
        cancel: CancellationToken,
        flow_deadline: Option<Instant>,
    ) -> Self {
        Self {
            engine,
            config,
            cancel,
            flow_deadline,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl super::flow_frame_engine::FrameStepExecutor for FlowTurnExecutorAdapter {
    async fn check_runtime_guards(&self, run_id: &RunId) -> Result<(), MobError> {
        if self.cancel.is_cancelled() {
            return Err(MobError::RunCanceled(run_id.clone()));
        }
        if let Some(deadline) = self.flow_deadline
            && Instant::now() >= deadline
        {
            return Err(MobError::FlowFailed {
                run_id: run_id.clone(),
                reason: flow_deadline_reason(deadline, run_id),
            });
        }
        Ok(())
    }

    async fn execute_step(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
        step_id: &StepId,
        context: &FlowContext,
    ) -> Result<super::flow_frame_engine::FrameStepResult, MobError> {
        let step = self
            .config
            .flow_spec
            .steps
            .get(step_id)
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "FlowTurnExecutorAdapter: no FlowStepSpec for step '{step_id}'"
                ))
            })?
            .clone();

        match self
            .engine
            .execute_step_with_all_guards(
                StepExecutionRequest {
                    run_id,
                    step_id,
                    step: &step,
                    context,
                    config: &self.config,
                },
                StepExecutionControl {
                    evaluate_condition_guard: true,
                    cancel: Some(&self.cancel),
                    flow_deadline: self.flow_deadline,
                },
            )
            .await?
        {
            StepGuardOutcome::Completed(output) => {
                Ok(super::flow_frame_engine::FrameStepResult::Completed(output))
            }
            StepGuardOutcome::Skipped { reason } => {
                let _ = (frame_id, node_id, reason);
                Ok(super::flow_frame_engine::FrameStepResult::Skipped)
            }
            StepGuardOutcome::Failed {
                reason,
                failure_ledger_recorded,
            } => {
                let _ = (frame_id, node_id);
                Ok(super::flow_frame_engine::FrameStepResult::Failed {
                    reason,
                    failure_ledger_recorded,
                })
            }
        }
    }
}

fn frame_step_projection_status_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
    frame_id: &FrameId,
    node_id: &FlowNodeId,
) -> Result<StepRunStatus, MobError> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let step_key = mob_dsl::StepId::from(step_id.as_str());
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    let node_key = mob_dsl::FlowNodeId::from(node_id.as_str());

    if machine_state.frame_run.get(&frame_key) != Some(&run_key) {
        return Err(MobError::Internal(format!(
            "project_frame_step_status: frame '{frame_id}' does not belong to run '{run_id}'"
        )));
    }
    if !machine_state
        .frame_tracked_nodes
        .get(&frame_key)
        .is_some_and(|nodes| nodes.contains(&node_key))
    {
        return Err(MobError::Internal(format!(
            "project_frame_step_status: frame '{frame_id}' does not track node '{node_id}'"
        )));
    }
    if machine_state
        .frame_node_step_ids
        .get(&frame_key)
        .and_then(|steps| steps.get(&node_key))
        != Some(&step_key)
    {
        return Err(MobError::Internal(format!(
            "project_frame_step_status: frame '{frame_id}' node '{node_id}' is not mapped to \
             step '{step_id}'"
        )));
    }
    let Some(status) = machine_state
        .frame_node_status
        .get(&frame_key)
        .and_then(|statuses| statuses.get(&node_key))
        .copied()
    else {
        return Err(MobError::Internal(format!(
            "project_frame_step_status: frame '{frame_id}' node '{node_id}' has no machine \
             status"
        )));
    };
    match status {
        mob_dsl::NodeRunStatus::Completed => Ok(StepRunStatus::Completed),
        mob_dsl::NodeRunStatus::Skipped => Ok(StepRunStatus::Skipped),
        mob_dsl::NodeRunStatus::Failed => Ok(StepRunStatus::Failed),
        other => Err(MobError::Internal(format!(
            "project_frame_step_status: frame '{frame_id}' node '{node_id}' has \
             non-projectable status {other:?}"
        ))),
    }
}

fn machine_run_step_status_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    step_id: &StepId,
) -> Result<Option<StepRunStatus>, MobError> {
    let key = mob_dsl::RunStepKey::from(format!("{run_id}\u{0}{}", step_id.as_str()));
    machine_state
        .run_step_status_flat
        .get(&key)
        .copied()
        .map(|status| match status {
            mob_dsl::StepRunStatus::Dispatched => Ok(StepRunStatus::Dispatched),
            mob_dsl::StepRunStatus::Completed => Ok(StepRunStatus::Completed),
            mob_dsl::StepRunStatus::Failed => Ok(StepRunStatus::Failed),
            mob_dsl::StepRunStatus::Skipped => Ok(StepRunStatus::Skipped),
            mob_dsl::StepRunStatus::Canceled => Ok(StepRunStatus::Canceled),
        })
        .transpose()
}

fn root_frame_terminal_phase_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
) -> Result<FlowFrameTerminalPhase, MobError> {
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    match machine_state.frame_phase.get(&frame_key).copied() {
        Some(mob_dsl::FrameStatus::Completed) => Ok(FlowFrameTerminalPhase::Completed),
        Some(mob_dsl::FrameStatus::Failed) => Ok(FlowFrameTerminalPhase::Failed),
        Some(mob_dsl::FrameStatus::Canceled) => Ok(FlowFrameTerminalPhase::Canceled),
        Some(mob_dsl::FrameStatus::Running) => Err(MobError::Internal(format!(
            "root frame '{frame_id}' has non-terminal MobMachine phase Running"
        ))),
        None => Err(MobError::Internal(format!(
            "root frame '{frame_id}' is missing MobMachine frame_phase"
        ))),
    }
}

fn flow_deadline_reason(deadline: Instant, run_id: &RunId) -> String {
    let elapsed = Instant::now().saturating_duration_since(deadline);
    let overrun_ms = elapsed.as_millis();
    if overrun_ms == 0 {
        format!("max flow duration exceeded for {run_id}")
    } else {
        format!("max flow duration exceeded for {run_id} (+{overrun_ms}ms)")
    }
}

fn effective_step_timeout(
    requested_step_timeout: Duration,
    flow_deadline: Option<Instant>,
    run_id: &RunId,
) -> Result<(Duration, Option<String>), MobError> {
    let Some(deadline) = flow_deadline else {
        return Ok((requested_step_timeout, None));
    };
    let now = Instant::now();
    if now >= deadline {
        return Err(MobError::FlowFailed {
            run_id: run_id.clone(),
            reason: flow_deadline_reason(deadline, run_id),
        });
    }

    let remaining = deadline.saturating_duration_since(now);
    if remaining <= requested_step_timeout {
        Ok((remaining, Some(flow_deadline_reason(deadline, run_id))))
    } else {
        Ok((requested_step_timeout, None))
    }
}

#[cfg(test)]
mod template_tests {
    use super::{
        TemplateSyntaxKind, parse_template, render_content_input_template, render_template,
    };
    use crate::ids::{RunId, StepId};
    use crate::run::FlowContext;
    use indexmap::IndexMap;
    use meerkat_core::types::{ContentBlock, ContentInput};

    fn sample_context() -> FlowContext {
        let mut step_outputs = IndexMap::new();
        step_outputs.insert(StepId::from("a"), serde_json::json!({"x": "ok"}));
        FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({"user":"luka"}),
            step_outputs,
            loop_outputs: IndexMap::new(),
        }
    }

    #[test]
    fn test_parse_template_reports_unmatched_close_with_location() {
        let error =
            parse_template("bad }}").expect_err("parser should fail unmatched close delimiter");
        assert_eq!(error.kind, TemplateSyntaxKind::UnmatchedClose);
        assert_eq!(error.line, 1);
        assert_eq!(error.column, 5);
    }

    #[test]
    fn test_parse_template_reports_nested_placeholder_open_with_location() {
        let error = parse_template("{{ params.{{nested}} }}")
            .expect_err("parser should fail nested placeholder open");
        assert_eq!(error.kind, TemplateSyntaxKind::NestedPlaceholderOpen);
        assert_eq!(error.line, 1);
    }

    #[test]
    fn test_render_template_renders_valid_expression_path() {
        let rendered = render_template("hello {{ params.user }}", &sample_context())
            .expect("valid template should render");
        assert_eq!(rendered, "hello luka");
    }

    #[test]
    fn test_render_content_input_template_preserves_non_text_blocks() {
        let rendered = render_content_input_template(
            &ContentInput::Blocks(vec![
                ContentBlock::Text {
                    text: "hello {{ params.user }}".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "abc".into(),
                },
            ]),
            &sample_context(),
        )
        .expect("valid multimodal template should render");

        match rendered {
            ContentInput::Blocks(blocks) => {
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Text { text } if text == "hello luka"
                ));
                assert!(matches!(
                    &blocks[1],
                    ContentBlock::Image {
                        media_type,
                        data,
                    } if media_type == "image/png"
                        && matches!(data, meerkat_core::ImageData::Inline { data } if data == "abc")
                ));
            }
            other => panic!("expected rendered blocks, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod strip_code_fences_tests {
    use super::strip_code_fences;

    #[test]
    fn plain_json_unchanged() {
        assert_eq!(strip_code_fences(r#"{"a":1}"#), r#"{"a":1}"#);
    }

    #[test]
    fn strips_json_fenced_block() {
        let input = "```json\n{\"narrative\": \"boom\"}\n```";
        assert_eq!(strip_code_fences(input), r#"{"narrative": "boom"}"#);
    }

    #[test]
    fn strips_plain_fenced_block() {
        let input = "```\n{\"a\":1}\n```";
        assert_eq!(strip_code_fences(input), r#"{"a":1}"#);
    }

    #[test]
    fn handles_whitespace_around_fences() {
        let input = "  \n```json\n{\"x\":2}\n```\n  ";
        assert_eq!(strip_code_fences(input), r#"{"x":2}"#);
    }

    #[test]
    fn extracts_first_fenced_block_even_with_leading_prose() {
        let input =
            "Here is the raw JSON response:\n\n```json\n{\"final_message\":\"joined\"}\n```";
        assert_eq!(strip_code_fences(input), r#"{"final_message":"joined"}"#);
    }
}

#[cfg(test)]
mod status_projection_tests {
    use super::{StepRunStatus, flow_run, step_status_from_flow_run};

    #[test]
    fn flow_run_step_status_projects_from_typed_variant() {
        assert_eq!(
            step_status_from_flow_run(flow_run::StepRunStatus::Failed),
            StepRunStatus::Failed
        );
        assert_eq!(
            step_status_from_flow_run(flow_run::StepRunStatus::Canceled),
            StepRunStatus::Canceled
        );
    }
}
