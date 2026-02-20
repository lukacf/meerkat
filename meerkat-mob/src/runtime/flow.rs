use super::flow_system_member_id;
use super::conditions::evaluate_condition;
use super::events::MobEventEmitter;
use super::handle::MobHandle;
use super::path::resolve_context_path;
use super::supervisor::Supervisor;
use super::terminalization::{
    FlowTerminalizationAuthority, TerminalizationOutcome, TerminalizationTarget,
};
use super::topology::{PolicyDecision, evaluate_topology};
use super::turn_executor::{FlowTurnExecutor, FlowTurnOutcome, TimeoutDisposition};
use crate::definition::{CollectionPolicy, DependencyMode, DispatchMode, FlowStepSpec, PolicyMode};
use crate::error::MobError;
use crate::ids::{BranchId, FlowId, MeerkatId, RunId, StepId};
use crate::run::{
    FailureLedgerEntry, FlowContext, FlowRunConfig, MobRunStatus, StepLedgerEntry, StepRunStatus,
};
use crate::store::{MobEventStore, MobRunStore};
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct FlowEngine {
    executor: Arc<dyn FlowTurnExecutor>,
    handle: MobHandle,
    run_store: Arc<dyn MobRunStore>,
    emitter: MobEventEmitter,
    terminalization: FlowTerminalizationAuthority,
}

impl FlowEngine {
    pub fn new(
        executor: Arc<dyn FlowTurnExecutor>,
        handle: MobHandle,
        run_store: Arc<dyn MobRunStore>,
        event_store: Arc<dyn MobEventStore>,
    ) -> Self {
        let terminalization = FlowTerminalizationAuthority::new(
            run_store.clone(),
            event_store.clone(),
            handle.mob_id().clone(),
        );
        let emitter = MobEventEmitter::new(event_store, handle.mob_id().clone());
        Self {
            executor,
            handle,
            run_store,
            emitter,
            terminalization,
        }
    }

    pub async fn execute_flow(
        &self,
        run_id: RunId,
        config: FlowRunConfig,
        activation_params: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<(), MobError> {
        let transitioned = self
            .run_store
            .cas_run_status(&run_id, MobRunStatus::Pending, MobRunStatus::Running)
            .await?;
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

        let ordered_steps = self.topological_steps(&config)?;
        let supervisor = Supervisor::new(self.handle.clone(), self.emitter.clone());
        let flow_started_at = Instant::now();
        let max_flow_duration = config
            .limits
            .as_ref()
            .and_then(|limits| limits.max_flow_duration_ms)
            .map(Duration::from_millis);
        let max_step_retries = config
            .limits
            .as_ref()
            .and_then(|limits| limits.max_step_retries)
            .unwrap_or(0);
        let mut context = FlowContext {
            run_id: run_id.clone(),
            activation_params,
            step_outputs: IndexMap::new(),
        };

        let mut branch_winners: BTreeMap<BranchId, StepId> = BTreeMap::new();
        let mut step_states: BTreeMap<StepId, StepRunStatus> = BTreeMap::new();
        let mut consecutive_failures: u32 = 0;
        let mut had_step_failures = false;
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

            if let Some(branch) = &step.branch
                && branch_winners.contains_key(branch)
            {
                self.record_step_skipped(
                    &run_id,
                    &step_id,
                    "branch winner already selected".to_string(),
                )
                .await?;
                step_states.insert(step_id.clone(), StepRunStatus::Skipped);
                continue;
            }

            if let Some(condition) = &step.condition
                && !evaluate_condition(condition, &context)
            {
                self.record_step_skipped(
                    &run_id,
                    &step_id,
                    "condition evaluated to false".to_string(),
                )
                .await?;
                step_states.insert(step_id.clone(), StepRunStatus::Skipped);
                continue;
            }

            match self.evaluate_dependencies(&step, &step_states) {
                Ok(DependencyDecision::Ready) => {}
                Ok(DependencyDecision::Skip(reason)) => {
                    self.record_step_skipped(&run_id, &step_id, reason).await?;
                    step_states.insert(step_id.clone(), StepRunStatus::Skipped);
                    continue;
                }
                Err(reason) => {
                    self.record_step_failed(&run_id, &step_id, reason.clone())
                        .await?;
                    step_states.insert(step_id.clone(), StepRunStatus::Failed);
                    had_step_failures = true;
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    self.maybe_escalate_supervisor(
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &reason,
                        consecutive_failures,
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
                    evaluate_topology(&topology.rules, from_role, &step.role),
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
                self.record_step_failed(&run_id, &step_id, reason.clone())
                    .await?;
                step_states.insert(step_id.clone(), StepRunStatus::Failed);
                had_step_failures = true;
                consecutive_failures = consecutive_failures.saturating_add(1);
                self.maybe_escalate_supervisor(
                    &supervisor,
                    &config,
                    &run_id,
                    &step_id,
                    &reason,
                    consecutive_failures,
                )
                .await?;
                continue;
            }

            let required_quorum = match step.collection_policy {
                CollectionPolicy::Quorum { n } => Some(n),
                _ => None,
            };
            if let Some(required) = required_quorum
                && usize::from(required) > target_count
            {
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

            let prompt = match render_template(&step.message, &context) {
                Ok(prompt) => prompt,
                Err(reason) => {
                    self.record_step_failed(&run_id, &step_id, reason.clone())
                        .await?;
                    step_states.insert(step_id.clone(), StepRunStatus::Failed);
                    had_step_failures = true;
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    self.maybe_escalate_supervisor(
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &reason,
                        consecutive_failures,
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
                in_flight.push(async move {
                    let result = engine
                        .execute_target_with_retries(
                            &run_id_for_target,
                            &step_id_for_target,
                            &target_for_task,
                            &prompt_for_task,
                            step_timeout,
                            max_step_retries,
                        )
                        .await;
                    (target_for_task, result)
                });
            }

            while let Some((target, target_result)) = in_flight.next().await {
                match target_result? {
                    TargetExecutionResult::Completed(output) => {
                        target_successes.push((target, output));
                    }
                    TargetExecutionResult::Failed(reason) => {
                        failure_reasons.push(reason);
                    }
                    TargetExecutionResult::Canceled => {
                        failure_reasons.push("turn canceled".to_string());
                    }
                }
            }

            let step_succeeded = collection_policy_satisfied(
                &step.collection_policy,
                target_count,
                target_successes.len(),
            );

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
                    self.record_step_failed(&run_id, &step_id, schema_error.to_string())
                        .await?;
                    step_states.insert(step_id.clone(), StepRunStatus::Failed);
                    had_step_failures = true;
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    self.maybe_escalate_supervisor(
                        &supervisor,
                        &config,
                        &run_id,
                        &step_id,
                        &schema_error.to_string(),
                        consecutive_failures,
                    )
                    .await?;
                    continue;
                }

                self.run_store
                    .append_step_entry(
                        &run_id,
                        StepLedgerEntry {
                            step_id: step_id.clone(),
                            meerkat_id: flow_system_member_id(),
                            status: StepRunStatus::Completed,
                            output: Some(aggregate_output.clone()),
                            timestamp: Utc::now(),
                        },
                    )
                    .await?;
                self.run_store
                    .put_step_output(&run_id, &step_id, aggregate_output.clone())
                    .await?;
                self.emitter
                    .step_completed(run_id.clone(), step_id.clone())
                    .await?;
                context
                    .step_outputs
                    .insert(step_id.clone(), aggregate_output);
                step_states.insert(step_id.clone(), StepRunStatus::Completed);
                if let Some(branch) = &step.branch {
                    branch_winners
                        .entry(branch.clone())
                        .or_insert_with(|| step_id.clone());
                }
                consecutive_failures = 0;
                continue;
            }

            let reason = if failure_reasons.is_empty() {
                "collection policy was not satisfied".to_string()
            } else {
                failure_reasons.join("; ")
            };

            self.record_step_failed(&run_id, &step_id, reason.clone())
                .await?;
            step_states.insert(step_id.clone(), StepRunStatus::Failed);
            had_step_failures = true;
            consecutive_failures = consecutive_failures.saturating_add(1);
            self.maybe_escalate_supervisor(
                &supervisor,
                &config,
                &run_id,
                &step_id,
                &reason,
                consecutive_failures,
            )
            .await?;
        }

        if canceled {
            let flow_id = config.flow_id;
            if let TerminalizationOutcome::Transitioned = self
                .terminalization
                .terminalize(run_id.clone(), flow_id, TerminalizationTarget::Canceled)
                .await?
            {
                tracing::debug!(run_id = %run_id, "flow canceled terminalization applied");
            }
            return Ok(());
        }

        if had_step_failures {
            let reason = "one or more flow steps failed".to_string();
            let flow_id = config.flow_id;
            if let TerminalizationOutcome::Transitioned = self
                .terminalization
                .terminalize(
                    run_id.clone(),
                    flow_id,
                    TerminalizationTarget::Failed { reason },
                )
                .await?
            {
                tracing::debug!(run_id = %run_id, "flow failed terminalization applied");
            }
            return Ok(());
        }

        let flow_id = config.flow_id;
        if let TerminalizationOutcome::Transitioned = self
            .terminalization
            .terminalize(run_id.clone(), flow_id, TerminalizationTarget::Completed)
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
        self.terminalization
            .terminalize(
                run_id.clone(),
                flow_id.clone(),
                TerminalizationTarget::Failed { reason },
            )
            .await?;
        Err(error)
    }

    async fn maybe_escalate_supervisor(
        &self,
        supervisor: &Supervisor,
        config: &FlowRunConfig,
        run_id: &RunId,
        step_id: &StepId,
        reason: &str,
        consecutive_failures: u32,
    ) -> Result<(), MobError> {
        let Some(supervisor_config) = config.supervisor.as_ref() else {
            return Ok(());
        };
        if supervisor_config.escalation_threshold == 0 {
            return Ok(());
        }
        if consecutive_failures < supervisor_config.escalation_threshold {
            return Ok(());
        }

        supervisor.escalate(config, run_id, step_id, reason).await?;
        supervisor.force_reset().await
    }

    async fn execute_target_with_retries(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        prompt: &str,
        step_timeout: Duration,
        max_retries: u32,
    ) -> Result<TargetExecutionResult, MobError> {
        for attempt in 0..=max_retries {
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
                .dispatch(run_id, step_id, target, prompt.to_string())
                .await?;

            match self
                .executor
                .await_terminal(ticket.clone(), step_timeout)
                .await
            {
                Ok(FlowTurnOutcome::Completed { output }) => {
                    let parsed_output = match parse_output_value(&output, step_id, target) {
                        Ok(parsed_output) => parsed_output,
                        Err(reason) => {
                            self.record_target_failure(run_id, step_id, target, reason.clone())
                                .await?;
                            if attempt < max_retries {
                                continue;
                            }
                            return Ok(TargetExecutionResult::Failed(reason));
                        }
                    };
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
                                output: Some(parsed_output.clone()),
                                timestamp: Utc::now(),
                            },
                        )
                        .await?;
                    return Ok(TargetExecutionResult::Completed(parsed_output));
                }
                Ok(FlowTurnOutcome::Failed { reason }) => {
                    self.record_target_failure(run_id, step_id, target, reason.clone())
                        .await?;
                    if attempt < max_retries {
                        continue;
                    }
                    return Ok(TargetExecutionResult::Failed(reason));
                }
                Ok(FlowTurnOutcome::Canceled) => {
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
                    if attempt < max_retries {
                        continue;
                    }
                    return Ok(TargetExecutionResult::Failed(timeout_reason));
                }
                Err(error) => {
                    let reason = error.to_string();
                    self.record_target_failure(run_id, step_id, target, reason.clone())
                        .await?;
                    if attempt < max_retries {
                        continue;
                    }
                    return Ok(TargetExecutionResult::Failed(reason));
                }
            }
        }

        Ok(TargetExecutionResult::Failed(
            "flow target retries exhausted".to_string(),
        ))
    }

    async fn record_target_failure(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target: &MeerkatId,
        reason: String,
    ) -> Result<(), MobError> {
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
        self.run_store
            .append_failure_entry(
                run_id,
                FailureLedgerEntry {
                    step_id: step_id.clone(),
                    reason,
                    timestamp: Utc::now(),
                },
            )
            .await
    }

    async fn select_targets(&self, step: &FlowStepSpec) -> Vec<MeerkatId> {
        let mut matching = self
            .handle
            .list_meerkats()
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

    fn topological_steps(&self, config: &FlowRunConfig) -> Result<Vec<StepId>, MobError> {
        let mut in_degree: BTreeMap<StepId, usize> = BTreeMap::new();
        let mut outgoing: BTreeMap<StepId, Vec<StepId>> = BTreeMap::new();

        for step_id in config.flow_spec.steps.keys() {
            in_degree.insert(step_id.clone(), 0);
            outgoing.entry(step_id.clone()).or_default();
        }

        for (step_id, step) in &config.flow_spec.steps {
            for dependency in &step.depends_on {
                if !in_degree.contains_key(dependency) {
                    return Err(MobError::Internal(format!(
                        "step '{step_id}' depends on unknown step '{dependency}'"
                    )));
                }
                *in_degree.entry(step_id.clone()).or_insert(0) += 1;
                outgoing
                    .entry(dependency.clone())
                    .or_default()
                    .push(step_id.clone());
            }
        }

        let mut queue = VecDeque::new();
        for step_id in config.flow_spec.steps.keys() {
            if in_degree.get(step_id) == Some(&0) {
                queue.push_back(step_id.clone());
            }
        }

        let mut ordered = Vec::with_capacity(config.flow_spec.steps.len());
        while let Some(next) = queue.pop_front() {
            ordered.push(next.clone());
            if let Some(children) = outgoing.get(&next) {
                for child in children {
                    if let Some(count) = in_degree.get_mut(child)
                        && *count > 0
                    {
                        *count -= 1;
                        if *count == 0 {
                            queue.push_back(child.clone());
                        }
                    }
                }
            }
        }

        if ordered.len() != config.flow_spec.steps.len() {
            return Err(MobError::Internal(
                "flow contains a cycle; cannot compute topological order".to_string(),
            ));
        }

        Ok(ordered)
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
        self.run_store
            .append_step_entry(
                run_id,
                StepLedgerEntry {
                    step_id: step_id.clone(),
                    meerkat_id: flow_system_member_id(),
                    status: StepRunStatus::Skipped,
                    output: None,
                    timestamp: Utc::now(),
                },
            )
            .await?;
        self.emitter
            .step_skipped(run_id.clone(), step_id.clone(), reason)
            .await?;
        Ok(())
    }

    async fn record_step_failed(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        reason: String,
    ) -> Result<(), MobError> {
        if !self.is_run_running(run_id).await? {
            return Ok(());
        }
        self.run_store
            .append_step_entry(
                run_id,
                StepLedgerEntry {
                    step_id: step_id.clone(),
                    meerkat_id: flow_system_member_id(),
                    status: StepRunStatus::Failed,
                    output: None,
                    timestamp: Utc::now(),
                },
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
        self.emitter
            .step_failed(run_id.clone(), step_id.clone(), reason)
            .await?;
        Ok(())
    }

    async fn is_run_running(&self, run_id: &RunId) -> Result<bool, MobError> {
        Ok(self
            .run_store
            .get_run(run_id)
            .await?
            .is_some_and(|run| run.status == MobRunStatus::Running))
    }

    fn evaluate_dependencies(
        &self,
        step: &FlowStepSpec,
        step_states: &BTreeMap<StepId, StepRunStatus>,
    ) -> Result<DependencyDecision, String> {
        if step.depends_on.is_empty() {
            return Ok(DependencyDecision::Ready);
        }

        match step.depends_on_mode {
            DependencyMode::All => {
                for dependency in &step.depends_on {
                    match step_states.get(dependency) {
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
                let mut completed = 0usize;
                let mut skipped = 0usize;
                for dependency in &step.depends_on {
                    match step_states.get(dependency) {
                        Some(StepRunStatus::Completed) => completed += 1,
                        Some(StepRunStatus::Skipped) => skipped += 1,
                        _ => {}
                    }
                }

                if completed > 0 {
                    Ok(DependencyDecision::Ready)
                } else if skipped == step.depends_on.len() {
                    Ok(DependencyDecision::Skip(
                        "all dependencies were skipped under depends_on_mode=any".to_string(),
                    ))
                } else {
                    Err(
                        "depends_on_mode=any requires at least one completed dependency"
                            .to_string(),
                    )
                }
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

fn parse_output_value(raw: &str, step_id: &StepId, target: &MeerkatId) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|error| {
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
            .map(|(_, output)| output.clone())
            .unwrap_or(Value::Null);
    }

    let mut output = Map::new();
    for (target, value) in successes {
        output.insert(target.as_str().to_string(), value.clone());
    }
    Value::Object(output)
}

fn collection_policy_satisfied(
    policy: &CollectionPolicy,
    target_count: usize,
    success_count: usize,
) -> bool {
    match policy {
        CollectionPolicy::All => success_count == target_count,
        CollectionPolicy::Any => success_count > 0,
        CollectionPolicy::Quorum { n } => success_count >= usize::from(*n),
    }
}

async fn validate_schema_ref(
    schema_ref: &str,
    step_id: &StepId,
    output: &Value,
) -> Result<(), MobError> {
    let schema = match serde_json::from_str::<Value>(schema_ref) {
        Ok(inline) => inline,
        Err(_) => {
            let raw = tokio::fs::read_to_string(schema_ref)
                .await
                .map_err(|error| MobError::SchemaValidation {
                    step_id: step_id.clone(),
                    message: format!("failed to read schema ref '{schema_ref}': {error}"),
                })?;
            serde_json::from_str(&raw).map_err(|error| MobError::SchemaValidation {
                step_id: step_id.clone(),
                message: format!("invalid schema JSON at '{schema_ref}': {error}"),
            })?
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
                "schema validator reported invalid output for step '{}' but yielded no errors",
                step_id
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

fn resolve_template_value<'a>(expression: &str, context: &'a FlowContext) -> Option<&'a Value> {
    resolve_context_path(context, expression)
}

#[cfg(test)]
mod template_tests {
    use super::{TemplateSyntaxKind, parse_template, render_template};
    use crate::ids::{RunId, StepId};
    use crate::run::FlowContext;
    use indexmap::IndexMap;

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
}
