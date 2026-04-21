use super::conditions::evaluate_condition;
use super::events::MobEventEmitter;
use super::flow_run_kernel::{
    FlowRunKernel, FlowRunMutator, FrameStepProjectionEffects, FrameStepProjectionRequest,
};
use super::flow_system_member_id;
use super::handle::MobHandle;
use super::path::resolve_context_path;
use super::supervisor::Supervisor;
use super::terminalization::TerminalizationOutcome;
use super::topology::{MobTopologyService, PolicyDecision};
use super::turn_executor::{FlowTurnExecutor, FlowTurnOutcome, TimeoutDisposition};
use crate::definition::{
    CollectionPolicy, DependencyMode, DispatchMode, FlowStepSpec, PolicyMode, StepOutputFormat,
};
use crate::error::MobError;
use crate::generated::flow_frame_loop_driver::FlowFrameTerminalPhase;
use crate::ids::{AgentIdentity, FlowId, MeerkatId, RunId, StepId};
use crate::run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, MobRunStatus, StepLedgerEntry, StepRunStatus,
};
use crate::store::{MobEventStore, MobRunStore};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::ContentInput;
use meerkat_machine_kernels::KernelEffect;
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct FlowEngine {
    executor: Arc<dyn FlowTurnExecutor>,
    handle: MobHandle,
    run_store: Arc<dyn MobRunStore>,
    emitter: MobEventEmitter,
    flow_kernel: FlowRunKernel,
    topology: Arc<MobTopologyService>,
}

impl FlowEngine {
    pub fn new(
        executor: Arc<dyn FlowTurnExecutor>,
        handle: MobHandle,
        run_store: Arc<dyn MobRunStore>,
        event_store: Arc<dyn MobEventStore>,
        topology: Arc<MobTopologyService>,
        flow_kernel: FlowRunKernel,
    ) -> Self {
        let emitter = MobEventEmitter::new(event_store, handle.mob_id().clone());
        Self {
            executor,
            handle,
            run_store,
            emitter,
            flow_kernel,
            topology,
        }
    }

    pub(crate) fn bind_topology_coordinator(&self) -> u32 {
        self.topology.bind_coordinator()
    }

    pub(crate) fn unbind_topology_coordinator(&self) -> u32 {
        self.topology.unbind_coordinator()
    }

    pub(crate) fn note_topology_spawn_boundary(&self) -> u32 {
        self.topology.note_spawn_boundary()
    }

    pub async fn execute_flow(
        &self,
        run_id: RunId,
        config: FlowRunConfig,
        activation_params: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<(), MobError> {
        let transitioned = self.flow_kernel.start_run(&run_id).await?;
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

        // If the flow uses the new frame-based execution path, dispatch to FlowFrameEngine.
        if let Some(root_spec) = &config.flow_spec.root {
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
                max_depth,
                max_active_frames,
            );
            let frame_result = frame_engine
                .execute_frame(&run_id, &frame_id, root_spec, &context)
                .await;

            match frame_result {
                Ok(frame_outcome) => {
                    let projected_run = self
                        .run_store
                        .get_run(&run_id)
                        .await?
                        .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;

                    if matches!(
                        frame_outcome.root_phase,
                        FlowFrameTerminalPhase::Failed | FlowFrameTerminalPhase::Canceled
                    ) {
                        if frame_outcome.root_phase == FlowFrameTerminalPhase::Failed {
                            for (step_id, status) in &frame_outcome.step_statuses {
                                if *status != StepRunStatus::Failed
                                    || run_step_has_terminal_projection(&projected_run, step_id)
                                {
                                    continue;
                                }
                                self.run_store
                                    .append_step_entry(
                                        &run_id,
                                        StepLedgerEntry {
                                            step_id: step_id.clone(),
                                            agent_identity: AgentIdentity::from(
                                                flow_system_member_id().as_str(),
                                            ),
                                            status: StepRunStatus::Failed,
                                            output: None,
                                            timestamp: Utc::now(),
                                        },
                                    )
                                    .await?;
                                self.run_store
                                    .append_failure_entry(
                                        &run_id,
                                        FailureLedgerEntry {
                                            step_id: step_id.clone(),
                                            reason: "frame execution marked step failed".into(),
                                            timestamp: Utc::now(),
                                        },
                                    )
                                    .await?;
                                self.emitter
                                    .step_failed(
                                        run_id.clone(),
                                        step_id.clone(),
                                        "frame execution marked step failed".into(),
                                    )
                                    .await?;
                            }
                        }
                        return self
                            .fail_run(
                                &run_id,
                                &config.flow_id,
                                MobError::FlowFailed {
                                    run_id: run_id.clone(),
                                    reason: match frame_outcome.root_phase {
                                        FlowFrameTerminalPhase::Failed => {
                                            "root frame sealed failed".into()
                                        }
                                        FlowFrameTerminalPhase::Canceled => {
                                            "root frame sealed canceled".into()
                                        }
                                        FlowFrameTerminalPhase::Completed => unreachable!(),
                                    },
                                },
                            )
                            .await;
                    }

                    // Project the typed frame outcome into FlowRunMachine once at the
                    // run/frame seam. This avoids reconstructing run truth from output
                    // side maps.
                    if let super::terminalization::TerminalizationOutcome::Transitioned = self
                        .flow_kernel
                        .terminalize_completed_from_frame(
                            &run_id,
                            config.flow_id.clone(),
                            &frame_outcome.step_statuses,
                        )
                        .await?
                    {
                        tracing::debug!(run_id = %run_id, "frame-based flow completed terminalization applied");
                    }

                    // Emit fallback skipped projections for frame-level auto-skips that
                    // never flowed through explicit step execution.
                    for (step_id, status) in &frame_outcome.step_statuses {
                        if *status == StepRunStatus::Skipped
                            && !run_step_has_terminal_projection(&projected_run, step_id)
                        {
                            self.run_store
                                .append_step_entry(
                                    &run_id,
                                    StepLedgerEntry {
                                        step_id: step_id.clone(),
                                        agent_identity: AgentIdentity::from(
                                            flow_system_member_id().as_str(),
                                        ),
                                        status: StepRunStatus::Skipped,
                                        output: None,
                                        timestamp: Utc::now(),
                                    },
                                )
                                .await?;
                            self.emitter
                                .step_skipped(
                                    run_id.clone(),
                                    step_id.clone(),
                                    "auto-skipped by frame execution".into(),
                                )
                                .await?;
                        }
                    }
                    for step_id in config.flow_spec.steps.keys() {
                        if !frame_outcome.step_statuses.contains_key(step_id)
                            && !run_step_has_terminal_projection(&projected_run, step_id)
                        {
                            self.run_store
                                .append_step_entry(
                                    &run_id,
                                    StepLedgerEntry {
                                        step_id: step_id.clone(),
                                        agent_identity: AgentIdentity::from(
                                            flow_system_member_id().as_str(),
                                        ),
                                        status: StepRunStatus::Skipped,
                                        output: None,
                                        timestamp: Utc::now(),
                                    },
                                )
                                .await?;
                            self.emitter
                                .step_skipped(
                                    run_id.clone(),
                                    step_id.clone(),
                                    "auto-skipped by frame execution".into(),
                                )
                                .await?;
                        }
                    }
                    return Ok(());
                }
                Err(e) => {
                    // Preserve the cancel/fail distinction (dogma Rule 4: one semantic
                    // condition, one canonical terminal path). The flat-step path calls
                    // terminalize_canceled when canceled — the frame path must too.
                    if matches!(e, MobError::RunCanceled(_)) {
                        self.flow_kernel.cancel_dispatched_steps(&run_id).await?;
                        if let super::terminalization::TerminalizationOutcome::Transitioned = self
                            .flow_kernel
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

        let ordered_steps = self.flow_kernel.ordered_steps(&run_id).await?;
        let supervisor = Supervisor::new(self.handle.clone(), self.emitter.clone());
        let max_flow_duration = config
            .limits
            .as_ref()
            .and_then(|limits| limits.max_flow_duration_ms)
            .map(Duration::from_millis);
        let mut context = FlowContext {
            run_id: run_id.clone(),
            activation_params,
            step_outputs: IndexMap::new(),
            loop_outputs: IndexMap::new(),
        };

        let mut canceled = false;

        for step_id in ordered_steps {
            if let Some(limit) = max_flow_duration
                && flow_started_at.elapsed() >= limit
            {
                return self
                    .fail_run(
                        &run_id,
                        &config.flow_id,
                        MobError::FlowFailed {
                            run_id: run_id.clone(),
                            reason: format!("max flow duration exceeded ({}ms)", limit.as_millis()),
                        },
                    )
                    .await;
            }

            if cancel.is_cancelled() {
                canceled = true;
                break;
            }

            if !self.is_run_running(&run_id).await? {
                canceled = true;
                break;
            }

            let step = config
                .flow_spec
                .steps
                .get(&step_id)
                .ok_or_else(|| {
                    MobError::Internal(format!("unknown flow step during execution: {step_id}"))
                })?
                .clone();

            if self
                .flow_kernel
                .step_branch_blocked(&run_id, &step_id)
                .await?
            {
                self.record_step_skipped(
                    &run_id,
                    &step_id,
                    "branch winner already selected".to_string(),
                )
                .await?;
                continue;
            }

            if let Some(condition) = &step.condition
                && !evaluate_condition(condition, &context)
            {
                let effects = self
                    .flow_kernel
                    .condition_rejected_effects(&run_id, &step_id)
                    .await?;
                self.apply_skip_projection(
                    effects,
                    &run_id,
                    &step_id,
                    "condition evaluated to false".to_string(),
                )
                .await?;
                continue;
            } else if step.condition.is_some() {
                let _ = self.flow_kernel.condition_passed(&run_id, &step_id).await?;
            }

            match self.evaluate_dependencies(&run_id, &step_id, &step).await {
                Ok(DependencyDecision::Ready) => {}
                Ok(DependencyDecision::Skip(reason)) => {
                    self.record_step_skipped(&run_id, &step_id, reason).await?;
                    continue;
                }
                Err(reason) => {
                    let should_escalate = self
                        .record_step_failed(&run_id, &step_id, reason.clone(), true)
                        .await?;
                    self.maybe_escalate_supervisor(
                        should_escalate,
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &reason,
                    )
                    .await?;
                    continue;
                }
            }

            // ── FlowRunMachine dispatch transition ──────────────────────
            let dispatch_effects = self
                .flow_kernel
                .dispatch_step_effects(&run_id, &step_id)
                .await?;
            let Some(dispatch_effects) = dispatch_effects else {
                continue;
            };
            if !has_effect(
                &dispatch_effects,
                FlowRunEffectKind::AdmitStepWork,
                Some(&step_id),
                None,
            ) {
                return Err(MobError::Internal(format!(
                    "flow_run DispatchStep did not emit AdmitStepWork for {step_id}"
                )));
            }

            // Cap the step timeout to the remaining flow duration so that
            // max_flow_duration_ms is honoured even when the step's own
            // timeout_ms is larger.
            let mut effective_step = step.clone();
            if let Some(limit) = max_flow_duration {
                let remaining = limit.saturating_sub(flow_started_at.elapsed());
                let step_timeout =
                    Duration::from_millis(effective_step.timeout_ms.unwrap_or(30_000))
                        .min(remaining);
                effective_step.timeout_ms = Some(step_timeout.as_millis() as u64);
            }

            // ── Canonical step execution ──────────────────────────────────
            match self
                .execute_step_with_all_guards(
                    StepExecutionRequest {
                        run_id: &run_id,
                        step_id: &step_id,
                        step: &effective_step,
                        context: &context,
                        config: &config,
                    },
                    StepExecutionControl {
                        evaluate_condition_guard: false,
                        cancel: Some(&cancel),
                        flow_deadline,
                    },
                )
                .await
            {
                Ok(StepGuardOutcome::Completed(output)) => {
                    let complete_effects = self
                        .flow_kernel
                        .complete_step_effects(&run_id, &step_id)
                        .await?;
                    let output_effects = self
                        .flow_kernel
                        .record_step_output_effects(&run_id, &step_id)
                        .await?;
                    if let Some(complete_effects) = complete_effects
                        && let Some(step_notice) =
                            find_step_notice_effect(&complete_effects, &step_id)?
                    {
                        // Ledger entry from FlowRunMachine; step_completed event
                        // is already emitted by execute_step_with_all_guards.
                        self.run_store
                            .append_step_entry(
                                &run_id,
                                StepLedgerEntry {
                                    step_id: step_notice.step_id.clone(),
                                    agent_identity: AgentIdentity::from(
                                        flow_system_member_id().as_str(),
                                    ),
                                    status: step_notice.status,
                                    output: Some(output.clone()),
                                    timestamp: Utc::now(),
                                },
                            )
                            .await?;
                    }
                    if let Some(output_effects) = output_effects
                        && has_effect(
                            &output_effects,
                            FlowRunEffectKind::PersistStepOutput,
                            Some(&step_id),
                            None,
                        )
                    {
                        self.run_store
                            .put_step_output(&run_id, &step_id, output.clone())
                            .await?;
                    }
                    context.step_outputs.insert(step_id.clone(), output);
                }
                Ok(StepGuardOutcome::Skipped { reason }) => {
                    // Condition was already checked before dispatch_step_effects,
                    // so Skipped here is unexpected. Treat as a soft failure in the
                    // FlowRunMachine (step was already dispatched, can't un-dispatch).
                    let should_escalate = self
                        .record_step_failed(&run_id, &step_id, reason.clone(), true)
                        .await?;
                    self.maybe_escalate_supervisor(
                        should_escalate,
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &reason,
                    )
                    .await?;
                }
                Ok(StepGuardOutcome::Failed {
                    reason,
                    failure_ledger_recorded,
                }) => {
                    let should_escalate = self
                        .record_step_failed(
                            &run_id,
                            &step_id,
                            reason.clone(),
                            !failure_ledger_recorded,
                        )
                        .await?;
                    self.maybe_escalate_supervisor(
                        should_escalate,
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &reason,
                    )
                    .await?;
                }
                Err(e) => {
                    // Infrastructure errors (e.g. orphan budget exhaustion,
                    // topology strict violations) should fail the entire run
                    // immediately, preserving the original reason.
                    if matches!(e, MobError::RunCanceled(_)) {
                        canceled = true;
                        break;
                    }
                    if matches!(
                        e,
                        MobError::FlowFailed { .. }
                            | MobError::TopologyViolation { .. }
                            | MobError::InsufficientTargets { .. }
                    ) {
                        return self.fail_run(&run_id, &config.flow_id, e).await;
                    }
                    let reason = e.to_string();
                    let should_escalate = self
                        .record_step_failed(&run_id, &step_id, reason.clone(), true)
                        .await?;
                    self.maybe_escalate_supervisor(
                        should_escalate,
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &reason,
                    )
                    .await?;
                }
            }
        }

        if canceled {
            let flow_id = config.flow_id;
            self.flow_kernel.cancel_dispatched_steps(&run_id).await?;
            if let TerminalizationOutcome::Transitioned = self
                .flow_kernel
                .terminalize_canceled(run_id.clone(), flow_id)
                .await?
            {
                tracing::debug!(run_id = %run_id, "flow canceled terminalization applied");
            }
            return Ok(());
        }

        if self.flow_kernel.failure_count(&run_id).await? > 0 {
            let reason = "one or more flow steps failed".to_string();
            let flow_id = config.flow_id;
            if let TerminalizationOutcome::Transitioned = self
                .flow_kernel
                .terminalize_failed(run_id.clone(), flow_id, reason)
                .await?
            {
                tracing::debug!(run_id = %run_id, "flow failed terminalization applied");
            }
            return Ok(());
        }

        let flow_id = config.flow_id;
        if let TerminalizationOutcome::Transitioned = self
            .flow_kernel
            .terminalize_completed(run_id.clone(), flow_id)
            .await?
        {
            tracing::debug!(run_id = %run_id, "flow completed terminalization applied");
        }

        Ok(())
    }

    /// Canonical step execution: condition -> topology -> targets -> dispatch ->
    /// retry -> schema -> ledger -> events -> supervisor -> FanOut aggregation.
    ///
    /// Does NOT touch FlowRunMachine or FlowFrameMachine state. It may emit
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
        let prompt = render_content_input_template(&step.message, context).map_err(|e| {
            MobError::Internal(format!("template render failed for step '{step_id}': {e}"))
        })?;

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
            Err(reason) => Ok(StepGuardOutcome::Failed {
                reason,
                failure_ledger_recorded: true,
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
        let prompt = render_content_input_template(&step.message, context).map_err(|e| {
            MobError::Internal(format!("template render failed for step '{step_id}': {e}"))
        })?;
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
        let mut failures: Vec<String> = Vec::new();

        while let Some((target, result)) = in_flight.next().await {
            match result {
                Err(e) => return Err(e),
                Ok(Ok(value)) => {
                    successes.push((target, value));
                }
                Ok(Err(reason)) => {
                    failures.push(reason);
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
                failures.join("; ")
            };
            return Ok(StepGuardOutcome::Failed {
                reason: combined,
                failure_ledger_recorded: !failures.is_empty(),
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
    /// `Ok(Err(reason))` on terminal target failure, and `Err(MobError)` on
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
    ) -> Result<Result<Value, String>, MobError> {
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

            let ticket = self
                .executor
                .dispatch(
                    run_id,
                    step_id,
                    target,
                    prompt.clone(),
                    flow_tool_overlay.clone(),
                )
                .await?;

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
                            return Ok(Err(reason));
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
                    return Ok(Err(reason));
                }
                Ok(FlowTurnOutcome::Canceled) => {
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
                    return Ok(Err(cancel_reason));
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
                    return Ok(Err(timeout_reason));
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
        self.flow_kernel.fail_dispatched_steps(run_id).await?;
        self.flow_kernel
            .terminalize_failed(run_id.clone(), flow_id.clone(), reason)
            .await?;
        Ok(())
    }

    async fn maybe_escalate_supervisor(
        &self,
        should_escalate: bool,
        supervisor: &Supervisor,
        config: &FlowRunConfig,
        run_id: &RunId,
        step_id: &StepId,
        reason: &str,
    ) -> Result<(), MobError> {
        if config.supervisor.is_none() {
            return Ok(());
        }
        if !should_escalate {
            return Ok(());
        }

        supervisor.escalate(config, run_id, step_id, reason).await?;
        supervisor.force_reset().await
    }

    async fn record_step_skipped(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        reason: String,
    ) -> Result<(), MobError> {
        if !self.is_run_running(run_id).await? {
            return Ok(());
        }
        let effects = self.flow_kernel.skip_step_effects(run_id, step_id).await?;
        self.apply_skip_projection(effects, run_id, step_id, reason)
            .await?;
        Ok(())
    }

    async fn record_step_failed(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        reason: String,
        append_failure_ledger: bool,
    ) -> Result<bool, MobError> {
        if !self.is_run_running(run_id).await? {
            return Ok(false);
        }
        let effects = self.flow_kernel.fail_step_effects(run_id, step_id).await?;
        self.apply_failure_projection(effects, run_id, step_id, reason, append_failure_ledger)
            .await
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
        effects: Option<Vec<KernelEffect>>,
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
        effects: Option<Vec<KernelEffect>>,
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

    async fn evaluate_dependencies(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        step: &FlowStepSpec,
    ) -> Result<DependencyDecision, String> {
        if step.depends_on.is_empty() {
            return Ok(DependencyDecision::Ready);
        }

        let ready = self
            .flow_kernel
            .step_dependency_ready(run_id, step_id)
            .await
            .map_err(|error| error.to_string())?;
        if ready {
            return Ok(DependencyDecision::Ready);
        }

        let should_skip = self
            .flow_kernel
            .step_dependency_should_skip(run_id, step_id)
            .await
            .map_err(|error| error.to_string())?;
        if should_skip {
            return Ok(DependencyDecision::Skip(
                "all dependencies were skipped under depends_on_mode=any".to_string(),
            ));
        }

        match step.depends_on_mode {
            DependencyMode::All => {
                for dependency in &step.depends_on {
                    match self
                        .flow_kernel
                        .step_status(run_id, dependency)
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        Some(StepRunStatus::Completed) => {}
                        Some(StepRunStatus::Skipped) => {
                            return Err(format!("dependency '{dependency}' was skipped"));
                        }
                        Some(_) => {
                            return Err(format!(
                                "dependency '{dependency}' did not complete successfully"
                            ));
                        }
                        None => {
                            return Err(format!("dependency '{dependency}' did not complete"));
                        }
                    }
                }
                Ok(DependencyDecision::Ready)
            }
            DependencyMode::Any => {
                Err("depends_on_mode=any requires at least one completed dependency".to_string())
            }
        }
    }
}

enum DependencyDecision {
    Ready,
    Skip(String),
}

/// Outcome of canonical step execution via `execute_step_with_all_guards`.
#[allow(dead_code)]
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
    fn parse(effect: &KernelEffect) -> Option<Self> {
        match effect.variant.as_str() {
            "AdmitStepWork" => Some(Self::AdmitStepWork),
            "EmitStepNotice" => Some(Self::EmitStepNotice),
            "PersistStepOutput" => Some(Self::PersistStepOutput),
            "AppendFailureLedger" => Some(Self::AppendFailureLedger),
            "EscalateSupervisor" => Some(Self::EscalateSupervisor),
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
    effects: &[KernelEffect],
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
    effects: &[KernelEffect],
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

fn effect_step_id(effect: &KernelEffect) -> Result<Option<StepId>, MobError> {
    let Some(value) = effect.fields.get("step_id") else {
        return Ok(None);
    };
    let step_id = value.as_string().map_err(|reason| {
        MobError::Internal(format!(
            "flow_run effect `{}` step_id invalid: {reason}",
            effect.variant
        ))
    })?;
    Ok(Some(StepId::from(step_id)))
}

fn effect_target_id(effect: &KernelEffect) -> Result<Option<MeerkatId>, MobError> {
    let Some(value) = effect.fields.get("target_id") else {
        return Ok(None);
    };
    let target_id = value.as_string().map_err(|reason| {
        MobError::Internal(format!(
            "flow_run effect `{}` target_id invalid: {reason}",
            effect.variant
        ))
    })?;
    Ok(Some(MeerkatId::from(target_id)))
}

fn effect_step_status(effect: &KernelEffect) -> Result<StepRunStatus, MobError> {
    let value = effect.fields.get("step_status").ok_or_else(|| {
        MobError::Internal(format!(
            "flow_run effect `{}` missing step_status payload",
            effect.variant
        ))
    })?;
    StepRunStatus::parse_kernel_value(value).map_err(|reason| {
        let message = if reason.starts_with("unknown StepRunStatus variant") {
            format!("flow_run effect `{}` {reason}", effect.variant)
        } else {
            format!(
                "flow_run effect `{}` step_status invalid: {reason}",
                effect.variant
            )
        };
        MobError::Internal(message)
    })
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
        _frame_id: &crate::ids::FrameId,
        _node_id: &crate::ids::FlowNodeId,
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
                let projection = self
                    .engine
                    .flow_kernel
                    .project_frame_step_status(
                        run_id,
                        step_id,
                        FrameStepProjectionRequest::completed(),
                    )
                    .await?;
                let _ = self
                    .engine
                    .apply_frame_step_projection(
                        projection,
                        run_id,
                        step_id,
                        Some(output.clone()),
                        None,
                    )
                    .await?;
                Ok(super::flow_frame_engine::FrameStepResult::Completed(output))
            }
            StepGuardOutcome::Skipped { reason } => {
                let projection = self
                    .engine
                    .flow_kernel
                    .project_frame_step_status(
                        run_id,
                        step_id,
                        FrameStepProjectionRequest::skipped(),
                    )
                    .await?;
                let _ = self
                    .engine
                    .apply_frame_step_projection(projection, run_id, step_id, None, Some(reason))
                    .await?;
                Ok(super::flow_frame_engine::FrameStepResult::Skipped)
            }
            StepGuardOutcome::Failed {
                reason,
                failure_ledger_recorded,
            } => {
                let projection = self
                    .engine
                    .flow_kernel
                    .project_frame_step_status(
                        run_id,
                        step_id,
                        FrameStepProjectionRequest::failed(!failure_ledger_recorded),
                    )
                    .await?;
                if self
                    .engine
                    .apply_frame_step_projection(
                        projection,
                        run_id,
                        step_id,
                        None,
                        Some(reason.clone()),
                    )
                    .await?
                {
                    let supervisor =
                        Supervisor::new(self.engine.handle.clone(), self.engine.emitter.clone());
                    supervisor
                        .escalate(&self.config, run_id, step_id, &reason)
                        .await?;
                    supervisor.force_reset().await?;
                }
                Ok(super::flow_frame_engine::FrameStepResult::Failed)
            }
        }
    }
}

fn run_step_has_terminal_projection(run: &crate::run::MobRun, step_id: &StepId) -> bool {
    run.step_ledger.iter().any(|entry| {
        &entry.step_id == step_id
            && matches!(
                entry.status,
                StepRunStatus::Completed | StepRunStatus::Skipped | StepRunStatus::Failed
            )
    })
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
