use super::conditions::evaluate_condition;
use super::events::MobEventEmitter;
use super::flow_run_kernel::{FlowRunKernel, FlowRunMutator};
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
use crate::ids::{FlowId, MeerkatId, RunId, StepId};
use crate::run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, MobRunStatus, StepLedgerEntry, StepRunStatus,
};
use crate::store::{MobEventStore, MobRunStore};
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

        let ordered_steps = self.flow_kernel.ordered_steps(&run_id).await?;
        let supervisor = Supervisor::new(self.handle.clone(), self.emitter.clone());
        let flow_started_at = Instant::now();
        let max_flow_duration = config
            .limits
            .as_ref()
            .and_then(|limits| limits.max_flow_duration_ms)
            .map(Duration::from_millis);
        let mut context = FlowContext {
            run_id: run_id.clone(),
            activation_params,
            step_outputs: IndexMap::new(),
        };

        let mut canceled = false;

        'steps: for step_id in ordered_steps {
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
                        .record_step_failed(&run_id, &step_id, reason.clone())
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

            if let Some(topology) = &config.topology {
                let from_role = config.orchestrator_role.as_ref().ok_or_else(|| {
                    MobError::Internal(
                        "topology is configured but flow run has no orchestrator role".to_string(),
                    )
                })?;
                if matches!(
                    self.topology.evaluate(from_role, &step.role),
                    PolicyDecision::Deny
                ) {
                    if matches!(topology.mode, PolicyMode::Strict) {
                        return self
                            .fail_run(
                                &run_id,
                                &config.flow_id,
                                MobError::TopologyViolation {
                                    from_role: from_role.clone(),
                                    to_role: step.role.clone(),
                                },
                            )
                            .await;
                    }

                    self.emitter
                        .topology_violation(from_role.clone(), step.role.clone())
                        .await?;
                }
            }

            let targets = self.select_targets(&step).await;
            let target_count = targets.len();
            if target_count == 0 {
                let reason = format!("no targets available for role '{}'", step.role);
                let should_escalate = self
                    .record_step_failed(&run_id, &step_id, reason.clone())
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

            let _ = self
                .flow_kernel
                .register_targets(&run_id, &step_id, target_count as u32)
                .await?;

            if !self
                .flow_kernel
                .collection_feasible(&run_id, &step_id)
                .await?
            {
                let required = match step.collection_policy {
                    CollectionPolicy::Quorum { n } => n,
                    _ => 0,
                };
                return self
                    .fail_run(
                        &run_id,
                        &config.flow_id,
                        MobError::InsufficientTargets {
                            step_id: step_id.clone(),
                            required,
                            available: target_count,
                        },
                    )
                    .await;
            }

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

            let prompt = match render_content_input_template(&step.message, &context) {
                Ok(prompt) => prompt,
                Err(reason) => {
                    let should_escalate = self
                        .record_step_failed(&run_id, &step_id, reason.clone())
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
            };
            let step_timeout = match max_flow_duration {
                Some(limit) => {
                    let remaining = limit.saturating_sub(flow_started_at.elapsed());
                    Duration::from_millis(step.timeout_ms.unwrap_or(30_000)).min(remaining)
                }
                None => Duration::from_millis(step.timeout_ms.unwrap_or(30_000)),
            };
            let mut target_successes: Vec<(MeerkatId, Value)> = Vec::new();
            let mut failure_reasons: Vec<String> = Vec::new();
            let mut in_flight = FuturesUnordered::new();
            let flow_tool_overlay = step_tool_overlay(&step);

            for target in targets {
                if cancel.is_cancelled() {
                    canceled = true;
                    break 'steps;
                }
                if !self.is_run_running(&run_id).await? {
                    canceled = true;
                    break 'steps;
                }

                let engine = self.clone();
                let run_id_for_target = run_id.clone();
                let step_id_for_target = step_id.clone();
                let target_for_task = target.clone();
                let prompt_for_task = prompt.clone();
                let flow_tool_overlay_for_task = flow_tool_overlay.clone();
                let output_format_for_task = step.output_format.clone();
                in_flight.push(async move {
                    let result = engine
                        .execute_target_with_retries(
                            &run_id_for_target,
                            &step_id_for_target,
                            &target_for_task,
                            &prompt_for_task,
                            flow_tool_overlay_for_task,
                            output_format_for_task,
                            step_timeout,
                        )
                        .await;
                    (target_for_task, result)
                });
            }

            while let Some((_target, target_result)) = in_flight.next().await {
                match target_result {
                    Err(error) => {
                        return self.fail_run(&run_id, &config.flow_id, error).await;
                    }
                    Ok(TargetExecutionResult::Completed(output)) => {
                        target_successes.push((_target, output));
                    }
                    Ok(TargetExecutionResult::Failed(reason)) => {
                        failure_reasons.push(reason);
                    }
                    Ok(TargetExecutionResult::Canceled) => {
                        failure_reasons.push("turn canceled".to_string());
                    }
                }
            }

            let step_succeeded = self
                .flow_kernel
                .collection_satisfied(&run_id, &step_id)
                .await?;

            if step_succeeded {
                target_successes.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
                let aggregate_output = aggregate_output(
                    &step.dispatch_mode,
                    &step.collection_policy,
                    &target_successes,
                );
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Err(schema_error) =
                        validate_schema_ref(schema_ref, &step_id, &aggregate_output).await
                {
                    let schema_reason = schema_error.to_string();
                    let should_escalate = self
                        .record_step_failed(&run_id, &step_id, schema_reason.clone())
                        .await?;
                    self.maybe_escalate_supervisor(
                        should_escalate,
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &schema_reason,
                    )
                    .await?;
                    continue;
                }

                let complete_effects = self
                    .flow_kernel
                    .complete_step_effects(&run_id, &step_id)
                    .await?;
                let output_effects = self
                    .flow_kernel
                    .record_step_output_effects(&run_id, &step_id)
                    .await?;
                if let Some(complete_effects) = complete_effects
                    && let Some(step_notice) = find_step_notice_effect(&complete_effects, &step_id)?
                {
                    self.run_store
                        .append_step_entry(
                            &run_id,
                            StepLedgerEntry {
                                step_id: step_notice.step_id.clone(),
                                meerkat_id: flow_system_member_id(),
                                status: step_notice.status,
                                output: Some(aggregate_output.clone()),
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    self.emitter
                        .step_completed(run_id.clone(), step_id.clone())
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
                        .put_step_output(&run_id, &step_id, aggregate_output.clone())
                        .await?;
                }
                context
                    .step_outputs
                    .insert(step_id.clone(), aggregate_output);
                continue;
            }

            let reason = if failure_reasons.is_empty() {
                "collection policy was not satisfied".to_string()
            } else {
                failure_reasons.join("; ")
            };

            let should_escalate = self
                .record_step_failed(&run_id, &step_id, reason.clone())
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

    #[allow(clippy::too_many_arguments)]
    async fn execute_target_with_retries(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        prompt: &ContentInput,
        flow_tool_overlay: Option<TurnToolOverlay>,
        output_format: StepOutputFormat,
        step_timeout: Duration,
    ) -> Result<TargetExecutionResult, MobError> {
        loop {
            self.emitter
                .step_dispatched(run_id.clone(), step_id.clone(), target.clone())
                .await?;
            self.run_store
                .append_step_entry_if_absent(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_id.clone(),
                        meerkat_id: target.clone(),
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

            match self
                .executor
                .await_terminal(ticket.clone(), step_timeout)
                .await
            {
                Ok(FlowTurnOutcome::Completed { output }) => {
                    let parsed_output =
                        match parse_output_value(&output, step_id, target, &output_format) {
                            Ok(parsed_output) => parsed_output,
                            Err(reason) => {
                                self.record_target_failure(run_id, step_id, target, reason.clone())
                                    .await?;
                                if self
                                    .flow_kernel
                                    .target_retry_allowed(run_id, step_id, target.as_str())
                                    .await?
                                {
                                    continue;
                                }
                                return Ok(TargetExecutionResult::Failed(reason));
                            }
                        };
                    let effects = self
                        .flow_kernel
                        .record_target_success_effects(run_id, step_id, target.as_str())
                        .await?;
                    self.apply_target_success_projection(
                        effects,
                        run_id,
                        step_id,
                        target,
                        &parsed_output,
                    )
                    .await?;
                    return Ok(TargetExecutionResult::Completed(parsed_output));
                }
                Ok(FlowTurnOutcome::Failed { reason }) => {
                    self.record_target_failure(run_id, step_id, target, reason.clone())
                        .await?;
                    if self
                        .flow_kernel
                        .target_retry_allowed(run_id, step_id, target.as_str())
                        .await?
                    {
                        continue;
                    }
                    let _ = self
                        .flow_kernel
                        .record_target_terminal_failure(run_id, step_id)
                        .await?;
                    return Ok(TargetExecutionResult::Failed(reason));
                }
                Ok(FlowTurnOutcome::Canceled) => {
                    let effects = self
                        .flow_kernel
                        .record_target_canceled_effects(run_id, step_id, target.as_str())
                        .await?;
                    self.apply_target_canceled_projection(effects, run_id, step_id, target)
                        .await?;
                    return Ok(TargetExecutionResult::Canceled);
                }
                Err(MobError::FlowTurnTimedOut) => {
                    let timeout_reason = match self.executor.on_timeout(ticket).await {
                        Ok(TimeoutDisposition::Detached) => {
                            format!("timeout after {}ms", step_timeout.as_millis())
                        }
                        Ok(TimeoutDisposition::Canceled) => {
                            format!("timeout + canceled after {}ms", step_timeout.as_millis())
                        }
                        Err(timeout_error) => {
                            return Err(MobError::FlowFailed {
                                run_id: run_id.clone(),
                                reason: timeout_error.to_string(),
                            });
                        }
                    };
                    self.record_target_failure(run_id, step_id, target, timeout_reason.clone())
                        .await?;
                    if self
                        .flow_kernel
                        .target_retry_allowed(run_id, step_id, target.as_str())
                        .await?
                    {
                        continue;
                    }
                    let _ = self
                        .flow_kernel
                        .record_target_terminal_failure(run_id, step_id)
                        .await?;
                    return Ok(TargetExecutionResult::Failed(timeout_reason));
                }
                Err(error) => {
                    let reason = error.to_string();
                    self.record_target_failure(run_id, step_id, target, reason.clone())
                        .await?;
                    if self
                        .flow_kernel
                        .target_retry_allowed(run_id, step_id, target.as_str())
                        .await?
                    {
                        continue;
                    }
                    let _ = self
                        .flow_kernel
                        .record_target_terminal_failure(run_id, step_id)
                        .await?;
                    return Ok(TargetExecutionResult::Failed(reason));
                }
            }
        }
    }

    async fn record_target_failure(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        reason: String,
    ) -> Result<(), MobError> {
        let effects = self
            .flow_kernel
            .record_target_failure_effects(run_id, step_id, target.as_str())
            .await?;
        self.apply_target_failure_projection(effects, run_id, step_id, target, reason)
            .await?;
        Ok(())
    }

    async fn select_targets(&self, step: &FlowStepSpec) -> Vec<MeerkatId> {
        let mut matching = self
            .handle
            .list_runnable_members()
            .await
            .into_iter()
            .filter(|entry| entry.profile == step.role)
            .map(|entry| entry.meerkat_id)
            .collect::<Vec<_>>();
        matching.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        match step.dispatch_mode {
            DispatchMode::OneToOne => matching.into_iter().take(1).collect(),
            DispatchMode::FanOut | DispatchMode::FanIn => matching,
        }
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
    ) -> Result<bool, MobError> {
        if !self.is_run_running(run_id).await? {
            return Ok(false);
        }
        let effects = self.flow_kernel.fail_step_effects(run_id, step_id).await?;
        self.apply_failure_projection(effects, run_id, step_id, reason)
            .await
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
                        meerkat_id: flow_system_member_id(),
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
                        meerkat_id: flow_system_member_id(),
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
        if has_effect(
            &effects,
            FlowRunEffectKind::AppendFailureLedger,
            Some(step_id),
            None,
        ) {
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

    async fn apply_target_success_projection(
        &self,
        effects: Option<Vec<KernelEffect>>,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        output: &Value,
    ) -> Result<(), MobError> {
        let Some(effects) = effects else {
            return Ok(());
        };
        if has_effect(
            &effects,
            FlowRunEffectKind::ProjectTargetSuccess,
            Some(step_id),
            Some(target),
        ) {
            self.emitter
                .step_target_completed(run_id.clone(), step_id.clone(), target.clone())
                .await?;
            self.run_store
                .append_step_entry(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_id.clone(),
                        meerkat_id: target.clone(),
                        status: StepRunStatus::Completed,
                        output: Some(output.clone()),
                        timestamp: Utc::now(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn apply_target_failure_projection(
        &self,
        effects: Option<Vec<KernelEffect>>,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        reason: String,
    ) -> Result<(), MobError> {
        let Some(effects) = effects else {
            return Ok(());
        };
        if has_effect(
            &effects,
            FlowRunEffectKind::ProjectTargetFailure,
            Some(step_id),
            Some(target),
        ) {
            self.emitter
                .step_target_failed(
                    run_id.clone(),
                    step_id.clone(),
                    target.clone(),
                    reason.clone(),
                )
                .await?;
            self.run_store
                .append_step_entry(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_id.clone(),
                        meerkat_id: target.clone(),
                        status: StepRunStatus::Failed,
                        output: None,
                        timestamp: Utc::now(),
                    },
                )
                .await?;
        }
        if has_effect(
            &effects,
            FlowRunEffectKind::AppendFailureLedger,
            Some(step_id),
            None,
        ) {
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
        Ok(())
    }

    async fn apply_target_canceled_projection(
        &self,
        effects: Option<Vec<KernelEffect>>,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
    ) -> Result<(), MobError> {
        let Some(effects) = effects else {
            return Ok(());
        };
        if has_effect(
            &effects,
            FlowRunEffectKind::ProjectTargetCanceled,
            Some(step_id),
            Some(target),
        ) {
            self.run_store
                .append_step_entry(
                    run_id,
                    StepLedgerEntry {
                        step_id: step_id.clone(),
                        meerkat_id: target.clone(),
                        status: StepRunStatus::Canceled,
                        output: None,
                        timestamp: Utc::now(),
                    },
                )
                .await?;
        }
        Ok(())
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

enum TargetExecutionResult {
    Completed(Value),
    Failed(String),
    Canceled,
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
    ProjectTargetSuccess,
    ProjectTargetFailure,
    ProjectTargetCanceled,
}

impl FlowRunEffectKind {
    fn parse(effect: &KernelEffect) -> Option<Self> {
        match effect.variant.as_str() {
            "AdmitStepWork" => Some(Self::AdmitStepWork),
            "EmitStepNotice" => Some(Self::EmitStepNotice),
            "PersistStepOutput" => Some(Self::PersistStepOutput),
            "AppendFailureLedger" => Some(Self::AppendFailureLedger),
            "EscalateSupervisor" => Some(Self::EscalateSupervisor),
            "ProjectTargetSuccess" => Some(Self::ProjectTargetSuccess),
            "ProjectTargetFailure" => Some(Self::ProjectTargetFailure),
            "ProjectTargetCanceled" => Some(Self::ProjectTargetCanceled),
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
    match value.as_named_variant("StepRunStatus") {
        Ok("Dispatched") => Ok(StepRunStatus::Dispatched),
        Ok("Completed") => Ok(StepRunStatus::Completed),
        Ok("Failed") => Ok(StepRunStatus::Failed),
        Ok("Skipped") => Ok(StepRunStatus::Skipped),
        Ok("Canceled") => Ok(StepRunStatus::Canceled),
        Ok(variant) => Err(MobError::Internal(format!(
            "flow_run effect `{}` unknown step_status `{variant}`",
            effect.variant
        ))),
        Err(reason) => Err(MobError::Internal(format!(
            "flow_run effect `{}` step_status invalid: {reason}",
            effect.variant
        ))),
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
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Skip optional language tag on the opening fence line
        let after_tag = rest.find('\n').map_or(rest, |i| &rest[i + 1..]);
        // Strip closing fence
        let body = after_tag
            .rfind("```")
            .map_or(after_tag, |i| &after_tag[..i]);
        body.trim().to_string()
    } else {
        trimmed.to_string()
    }
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
                    data: "abc".to_string(),
                    source_path: Some("/tmp/source.png".to_string()),
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
                        source_path,
                    } if media_type == "image/png"
                        && data == "abc"
                        && source_path.as_deref() == Some("/tmp/source.png")
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
}
