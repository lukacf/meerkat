use super::conditions::evaluate_condition;
use super::events::MobEventEmitter;
use super::handle::MobHandle;
use super::supervisor::Supervisor;
use super::topology::{PolicyDecision, evaluate_topology};
use super::turn_executor::{FlowTurnExecutor, FlowTurnOutcome, TimeoutDisposition};
use crate::definition::{CollectionPolicy, DependencyMode, DispatchMode, FlowStepSpec, PolicyMode};
use crate::error::MobError;
use crate::ids::{FlowId, MeerkatId, RunId, StepId};
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
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const FLOW_SYSTEM_MEMBER_ID: &str = "__flow__";

#[derive(Clone)]
pub struct FlowEngine {
    executor: Arc<dyn FlowTurnExecutor>,
    handle: MobHandle,
    run_store: Arc<dyn MobRunStore>,
    emitter: MobEventEmitter,
}

impl FlowEngine {
    pub fn new(
        executor: Arc<dyn FlowTurnExecutor>,
        handle: MobHandle,
        run_store: Arc<dyn MobRunStore>,
        event_store: Arc<dyn MobEventStore>,
    ) -> Self {
        let emitter = MobEventEmitter::new(event_store, handle.mob_id().clone());
        Self {
            executor,
            handle,
            run_store,
            emitter,
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
        let mut context = FlowContext {
            run_id: run_id.clone(),
            activation_params,
            step_outputs: IndexMap::new(),
        };

        let mut branch_winners: BTreeMap<String, StepId> = BTreeMap::new();
        let mut step_states: BTreeMap<StepId, StepRunStatus> = BTreeMap::new();
        let mut consecutive_failures: u32 = 0;
        let mut had_step_failures = false;
        let mut canceled = false;

        'steps: for step_id in ordered_steps {
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
                .get(step_id.as_str())
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
                DependencyDecision::Ready => {}
                DependencyDecision::Skip(reason) => {
                    self.record_step_skipped(&run_id, &step_id, reason).await?;
                    step_states.insert(step_id.clone(), StepRunStatus::Skipped);
                    continue;
                }
                DependencyDecision::Fail(reason) => {
                    self.record_step_failed(&run_id, &step_id, reason.clone()).await?;
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

            if let Some(branch) = &step.branch {
                branch_winners
                    .entry(branch.clone())
                    .or_insert_with(|| step_id.clone());
            }

            if let Some(topology) = &config.topology
                && let Some(from_role) = config.orchestrator_role.as_deref()
                && matches!(
                    evaluate_topology(&topology.rules, from_role, &step.role),
                    PolicyDecision::Deny
                )
            {
                if matches!(topology.mode, PolicyMode::Strict) {
                    return self
                        .fail_run(
                            &run_id,
                            &config.flow_id,
                            MobError::TopologyViolation {
                                from_role: from_role.to_string(),
                                to_role: step.role.clone(),
                            },
                        )
                        .await;
                }

                self.emitter
                    .topology_violation(from_role.to_string(), step.role.clone())
                    .await?;
            }

            let targets = self.select_targets(&step).await;
            let target_count = targets.len();
            if target_count == 0 {
                let reason = format!("no targets available for role '{}'", step.role);
                self.record_step_failed(&run_id, &step_id, reason.clone()).await?;
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

            let prompt = render_template(&step.message, &context);
            let step_timeout = Duration::from_millis(step.timeout_ms.unwrap_or(30_000));
            let mut target_successes: Vec<(MeerkatId, Value)> = Vec::new();
            let mut failure_reasons: Vec<String> = Vec::new();
            let mut pending_turns = Vec::with_capacity(target_count);

            for target in targets {
                if cancel.is_cancelled() {
                    canceled = true;
                    break 'steps;
                }
                if !self.is_run_running(&run_id).await? {
                    canceled = true;
                    break 'steps;
                }

                self.emitter
                    .step_dispatched(run_id.clone(), step_id.clone(), target.clone())
                    .await?;
                self.run_store
                    .append_step_entry_if_absent(
                        &run_id,
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
                    .dispatch(&run_id, &step_id, &target, prompt.clone())
                    .await?;
                pending_turns.push((target, ticket));
            }

            let mut in_flight = FuturesUnordered::new();
            for (target, ticket) in pending_turns {
                let executor = self.executor.clone();
                let timeout_ticket = ticket.clone();
                in_flight.push(async move {
                    let terminal = executor.await_terminal(ticket, step_timeout).await;
                    (target, timeout_ticket, terminal)
                });
            }

            while let Some((target, timeout_ticket, terminal)) = in_flight.next().await {
                match terminal {
                    Ok(FlowTurnOutcome::Completed { output }) => {
                        let parsed_output = parse_output_value(&output);
                        self.emitter
                            .step_target_completed(run_id.clone(), step_id.clone(), target.clone())
                            .await?;
                        self.run_store
                            .append_step_entry(
                                &run_id,
                                StepLedgerEntry {
                                    step_id: step_id.clone(),
                                    meerkat_id: target.clone(),
                                    status: StepRunStatus::Completed,
                                    output: Some(parsed_output.clone()),
                                    timestamp: Utc::now(),
                                },
                            )
                            .await?;
                        target_successes.push((target, parsed_output));
                    }
                    Ok(FlowTurnOutcome::Failed { reason }) => {
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
                                &run_id,
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
                                &run_id,
                                FailureLedgerEntry {
                                    step_id: step_id.clone(),
                                    reason: reason.clone(),
                                    timestamp: Utc::now(),
                                },
                            )
                            .await?;
                        failure_reasons.push(reason);
                    }
                    Ok(FlowTurnOutcome::Canceled) => {
                        let reason = "turn canceled".to_string();
                        self.run_store
                            .append_step_entry(
                                &run_id,
                                StepLedgerEntry {
                                    step_id: step_id.clone(),
                                    meerkat_id: target.clone(),
                                    status: StepRunStatus::Canceled,
                                    output: None,
                                    timestamp: Utc::now(),
                                },
                            )
                            .await?;
                        failure_reasons.push(reason);
                    }
                    Err(error) if is_timeout_error(&error) => {
                        let timeout_reason = match self.executor.on_timeout(timeout_ticket).await {
                            Ok(TimeoutDisposition::Detached) => {
                                format!("timeout after {}ms", step_timeout.as_millis())
                            }
                            Ok(TimeoutDisposition::Canceled) => {
                                format!("timeout + canceled after {}ms", step_timeout.as_millis())
                            }
                            Err(timeout_error) => {
                                return self
                                    .fail_run(
                                        &run_id,
                                        &config.flow_id,
                                        MobError::FlowFailed {
                                            run_id: run_id.clone(),
                                            reason: timeout_error.to_string(),
                                        },
                                    )
                                    .await;
                            }
                        };

                        self.emitter
                            .step_target_failed(
                                run_id.clone(),
                                step_id.clone(),
                                target.clone(),
                                timeout_reason.clone(),
                            )
                            .await?;
                        self.run_store
                            .append_step_entry(
                                &run_id,
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
                                &run_id,
                                FailureLedgerEntry {
                                    step_id: step_id.clone(),
                                    reason: timeout_reason.clone(),
                                    timestamp: Utc::now(),
                                },
                            )
                            .await?;
                        failure_reasons.push(timeout_reason);
                    }
                    Err(error) => {
                        let reason = error.to_string();
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
                                &run_id,
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
                                &run_id,
                                FailureLedgerEntry {
                                    step_id: step_id.clone(),
                                    reason: reason.clone(),
                                    timestamp: Utc::now(),
                                },
                            )
                            .await?;
                        failure_reasons.push(reason);
                    }
                }
            }

            let step_succeeded = collection_policy_satisfied(
                &step.collection_policy,
                target_count,
                target_successes.len(),
            );

            if step_succeeded {
                let aggregate_output = aggregate_output(&step.collection_policy, &target_successes);
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Err(schema_error) =
                        validate_schema_ref(schema_ref, &step_id, &aggregate_output)
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
                            meerkat_id: MeerkatId::from(FLOW_SYSTEM_MEMBER_ID),
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
                context.step_outputs.insert(step_id.clone(), aggregate_output);
                step_states.insert(step_id.clone(), StepRunStatus::Completed);
                consecutive_failures = 0;
                continue;
            }

            let reason = if failure_reasons.is_empty() {
                "collection policy was not satisfied".to_string()
            } else {
                failure_reasons.join("; ")
            };

            self.record_step_failed(&run_id, &step_id, reason.clone()).await?;
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
            let finalized = self
                .run_store
                .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Canceled)
                .await?;
            if finalized {
                self.emitter
                    .flow_canceled(run_id, config.flow_id)
                    .await?;
            }
            return Ok(());
        }

        if had_step_failures {
            let reason = "one or more flow steps failed".to_string();
            let finalized = self
                .run_store
                .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Failed)
                .await?;
            if finalized {
                self.emitter
                    .flow_failed(run_id.clone(), config.flow_id, reason)
                    .await?;
            }
            return Ok(());
        }

        let finalized = self
            .run_store
            .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Completed)
            .await?;
        if finalized {
            self.emitter
                .flow_completed(run_id, config.flow_id)
                .await?;
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
        let finalized = self
            .run_store
            .cas_run_status(run_id, MobRunStatus::Running, MobRunStatus::Failed)
            .await?;
        if finalized {
            self.emitter
                .flow_failed(run_id.clone(), flow_id.clone(), reason)
                .await?;
        }
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

    async fn select_targets(&self, step: &FlowStepSpec) -> Vec<MeerkatId> {
        let mut matching = self
            .handle
            .list_meerkats()
            .await
            .into_iter()
            .filter(|entry| entry.profile.as_str() == step.role)
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
            let typed_id = StepId::from(step_id.as_str());
            in_degree.insert(typed_id.clone(), 0);
            outgoing.entry(typed_id).or_default();
        }

        for (step_id, step) in &config.flow_spec.steps {
            let typed_id = StepId::from(step_id.as_str());
            for dependency in &step.depends_on {
                let dependency_id = StepId::from(dependency.as_str());
                if !in_degree.contains_key(&dependency_id) {
                    return Err(MobError::Internal(format!(
                        "step '{step_id}' depends on unknown step '{dependency}'"
                    )));
                }
                *in_degree.entry(typed_id.clone()).or_insert(0) += 1;
                outgoing.entry(dependency_id).or_default().push(typed_id.clone());
            }
        }

        let mut queue = VecDeque::new();
        for step_id in config.flow_spec.steps.keys() {
            let typed = StepId::from(step_id.as_str());
            if in_degree.get(&typed) == Some(&0) {
                queue.push_back(typed);
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
                    meerkat_id: MeerkatId::from(FLOW_SYSTEM_MEMBER_ID),
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
                    meerkat_id: MeerkatId::from(FLOW_SYSTEM_MEMBER_ID),
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
    ) -> DependencyDecision {
        if step.depends_on.is_empty() {
            return DependencyDecision::Ready;
        }

        match step.depends_on_mode {
            DependencyMode::All => {
                for dependency in &step.depends_on {
                    let dependency_id = StepId::from(dependency.as_str());
                    match step_states.get(&dependency_id) {
                        Some(StepRunStatus::Completed) => {}
                        Some(StepRunStatus::Skipped) => {
                            return DependencyDecision::Fail(format!(
                                "dependency '{dependency}' was skipped"
                            ));
                        }
                        Some(_) => {
                            return DependencyDecision::Fail(format!(
                                "dependency '{dependency}' did not complete successfully"
                            ));
                        }
                        None => {
                            return DependencyDecision::Fail(format!(
                                "dependency '{dependency}' did not complete"
                            ));
                        }
                    }
                }
                DependencyDecision::Ready
            }
            DependencyMode::Any => {
                let mut completed = 0usize;
                let mut skipped = 0usize;
                for dependency in &step.depends_on {
                    let dependency_id = StepId::from(dependency.as_str());
                    match step_states.get(&dependency_id) {
                        Some(StepRunStatus::Completed) => completed += 1,
                        Some(StepRunStatus::Skipped) => skipped += 1,
                        _ => {}
                    }
                }

                if completed > 0 {
                    DependencyDecision::Ready
                } else if skipped == step.depends_on.len() {
                    DependencyDecision::Skip(
                        "all dependencies were skipped under depends_on_mode=any".to_string(),
                    )
                } else {
                    DependencyDecision::Fail(
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
    Fail(String),
}

fn parse_output_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn aggregate_output(policy: &CollectionPolicy, successes: &[(MeerkatId, Value)]) -> Value {
    if successes.len() == 1 {
        return successes[0].1.clone();
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

fn validate_schema_ref(schema_ref: &str, step_id: &StepId, output: &Value) -> Result<(), MobError> {
    let schema = match serde_json::from_str::<Value>(schema_ref) {
        Ok(inline) => inline,
        Err(_) => {
            let raw = std::fs::read_to_string(schema_ref).map_err(|error| {
                MobError::SchemaValidation {
                    step_id: step_id.clone(),
                    message: format!("failed to read schema ref '{schema_ref}': {error}"),
                }
            })?;
            serde_json::from_str(&raw).map_err(|error| MobError::SchemaValidation {
                step_id: step_id.clone(),
                message: format!("invalid schema JSON at '{schema_ref}': {error}"),
            })?
        }
    };
    let validator = jsonschema::validator_for(&schema).map_err(|error| MobError::SchemaValidation {
        step_id: step_id.clone(),
        message: format!("schema compile failed: {error}"),
    })?;

    if validator.is_valid(output) {
        return Ok(());
    }

    let message = validator
        .iter_errors(output)
        .next()
        .map(|error| error.to_string())
        .unwrap_or_else(|| "schema validation failed".to_string());

    Err(MobError::SchemaValidation {
        step_id: step_id.clone(),
        message,
    })
}

fn is_timeout_error(error: &MobError) -> bool {
    matches!(error, MobError::FlowTurnTimedOut)
}

fn render_template(template: &str, context: &FlowContext) -> String {
    let mut rendered = String::new();
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find("}}") else {
            rendered.push_str(&rest[start..]);
            return rendered;
        };

        let expression = rest[start + 2..start + 2 + end].trim();
        let replacement = resolve_template_value(expression, context)
            .map_or(Value::Null, Clone::clone);

        match replacement {
            Value::String(text) => rendered.push_str(&text),
            other => rendered.push_str(&other.to_string()),
        }

        rest = &rest[start + 2 + end + 2..];
    }

    rendered.push_str(rest);
    rendered
}

fn resolve_template_value<'a>(expression: &str, context: &'a FlowContext) -> Option<&'a Value> {
    let mut parts = expression.split('.');
    match parts.next()? {
        "params" => walk_json(&context.activation_params, parts),
        "steps" => {
            let step_id = parts.next()?;
            let output = context.step_outputs.get(step_id)?;
            if parts.clone().next() == Some("output") {
                let _ = parts.next();
            }
            walk_json(output, parts)
        }
        _ => None,
    }
}

fn walk_json<I>(root: &Value, parts: I) -> Option<&Value>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut current = root;
    for segment in parts {
        let segment = segment.as_ref();
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => {
                let index: usize = segment.parse().ok()?;
                items.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}
