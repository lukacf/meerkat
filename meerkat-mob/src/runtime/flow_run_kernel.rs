use super::terminalization::{
    FlowTerminalizationAuthority, TerminalizationOutcome, TerminalizationTarget,
};
use crate::error::MobError;
use crate::ids::{FlowId, MobId, RunId, StepId};
use crate::run::{FlowRunConfig, MobRun, MobRunStatus};
use crate::store::{MobEventStore, MobRunStore};
use meerkat_machine_kernels::generated::flow_run;
use meerkat_machine_kernels::{
    KernelEffect, KernelInput, KernelState, KernelValue, TransitionOutcome,
};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct FlowRunKernel {
    mob_id: MobId,
    run_store: Arc<dyn MobRunStore>,
    terminalization: FlowTerminalizationAuthority,
}

impl FlowRunKernel {
    fn step_id_value(step_id: &StepId) -> KernelValue {
        KernelValue::String(step_id.to_string())
    }

    fn target_id_value(target_id: &str) -> KernelValue {
        KernelValue::String(target_id.to_string())
    }

    fn retry_key(step_id: &StepId, target_id: &str) -> String {
        format!("{step_id}::{target_id}")
    }

    pub fn new(
        mob_id: MobId,
        run_store: Arc<dyn MobRunStore>,
        events: Arc<dyn MobEventStore>,
    ) -> Self {
        let terminalization =
            FlowTerminalizationAuthority::new(run_store.clone(), events, mob_id.clone());
        Self {
            mob_id,
            run_store,
            terminalization,
        }
    }

    pub async fn create_pending_run(
        &self,
        config: &FlowRunConfig,
        activation_params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        let flow_state = MobRun::flow_state_for_config(config)?;
        let run = MobRun::pending(
            self.mob_id.clone(),
            config.flow_id.clone(),
            flow_state,
            activation_params,
        );
        let run_id = run.run_id.clone();
        self.run_store.create_run(run).await?;
        Ok(run_id)
    }

    pub async fn start_run(&self, run_id: &RunId) -> Result<bool, MobError> {
        let run = self.require_run(run_id).await?;
        if run.status != MobRunStatus::Pending {
            return Ok(false);
        }
        let next_state = self.transition_state(&run.flow_state, "StartRun", BTreeMap::new())?;
        self.run_store
            .cas_run_snapshot(
                run_id,
                MobRunStatus::Pending,
                &run.flow_state,
                MobRunStatus::Running,
                &next_state,
            )
            .await
    }

    fn terminal_variant(target: &TerminalizationTarget) -> &'static str {
        match target {
            TerminalizationTarget::Completed => "TerminalizeCompleted",
            TerminalizationTarget::Failed { .. } => "TerminalizeFailed",
            TerminalizationTarget::Canceled => "TerminalizeCanceled",
        }
    }

    async fn terminalize(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        target: TerminalizationTarget,
    ) -> Result<TerminalizationOutcome, MobError> {
        let run = self.require_run(&run_id).await?;
        let next_status = target.status();
        let next_state = self.transition_state(
            &run.flow_state,
            Self::terminal_variant(&target),
            BTreeMap::new(),
        )?;
        let transitioned = self
            .run_store
            .cas_run_snapshot(
                &run_id,
                run.status.clone(),
                &run.flow_state,
                next_status,
                &next_state,
            )
            .await?;
        if !transitioned {
            return Ok(TerminalizationOutcome::Noop);
        }
        self.terminalization
            .record_persisted_terminalization(run_id, flow_id, target)
            .await
    }

    pub async fn dispatch_step(&self, run_id: &RunId, step_id: &StepId) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "DispatchStep", step_id).await
    }

    pub async fn dispatch_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        self.apply_step_input_with_effects(run_id, "DispatchStep", step_id)
            .await
    }

    pub async fn complete_step(&self, run_id: &RunId, step_id: &StepId) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "CompleteStep", step_id).await
    }

    pub async fn complete_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        self.apply_step_input_with_effects(run_id, "CompleteStep", step_id)
            .await
    }

    pub async fn record_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "RecordStepOutput", step_id)
            .await
    }

    pub async fn record_step_output_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        self.apply_step_input_with_effects(run_id, "RecordStepOutput", step_id)
            .await
    }

    pub async fn condition_passed(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "ConditionPassed", step_id)
            .await
    }

    pub async fn condition_rejected(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "ConditionRejected", step_id)
            .await
    }

    pub async fn condition_rejected_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        self.apply_step_input_with_effects(run_id, "ConditionRejected", step_id)
            .await
    }

    pub async fn fail_step(&self, run_id: &RunId, step_id: &StepId) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "FailStep", step_id).await
    }

    pub async fn fail_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        self.apply_step_input_with_effects(run_id, "FailStep", step_id)
            .await
    }

    pub async fn skip_step(&self, run_id: &RunId, step_id: &StepId) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "SkipStep", step_id).await
    }

    pub async fn cancel_step(&self, run_id: &RunId, step_id: &StepId) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "CancelStep", step_id).await
    }

    pub async fn skip_step_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        self.apply_step_input_with_effects(run_id, "SkipStep", step_id)
            .await
    }

    async fn apply_step_input(
        &self,
        run_id: &RunId,
        variant: &str,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        Ok(self
            .apply_step_input_with_effects(run_id, variant, step_id)
            .await?
            .is_some())
    }

    async fn apply_step_input_with_effects(
        &self,
        run_id: &RunId,
        variant: &str,
        step_id: &StepId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        let run = self.require_run(run_id).await?;
        let outcome = self.transition_outcome(
            &run.flow_state,
            variant,
            BTreeMap::from([(
                "step_id".to_string(),
                KernelValue::String(step_id.to_string()),
            )]),
        )?;
        let transitioned = self
            .run_store
            .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
            .await?;
        if transitioned {
            Ok(Some(outcome.effects))
        } else {
            Ok(None)
        }
    }

    async fn require_run(&self, run_id: &RunId) -> Result<MobRun, MobError> {
        self.run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))
    }

    pub async fn snapshot(&self, run_id: &RunId) -> Result<MobRun, MobError> {
        self.require_run(run_id).await
    }

    fn transition_outcome(
        &self,
        state: &KernelState,
        variant: &str,
        fields: BTreeMap<String, KernelValue>,
    ) -> Result<TransitionOutcome, MobError> {
        flow_run::transition(
            state,
            &KernelInput {
                variant: variant.to_string(),
                fields,
            },
        )
        .map_err(|error| {
            MobError::Internal(format!("flow_run {variant} transition refused: {error}"))
        })
    }

    fn transition_state(
        &self,
        state: &KernelState,
        variant: &str,
        fields: BTreeMap<String, KernelValue>,
    ) -> Result<KernelState, MobError> {
        Ok(self.transition_outcome(state, variant, fields)?.next_state)
    }

    fn evaluate_helper_value(
        &self,
        state: &KernelState,
        helper: &str,
        fields: BTreeMap<String, KernelValue>,
    ) -> Result<KernelValue, MobError> {
        flow_run::kernel()
            .evaluate_helper(state, helper, &fields)
            .map_err(|error| {
                MobError::Internal(format!("flow_run helper {helper} refused: {error}"))
            })
    }

    async fn helper_bool(
        &self,
        run_id: &RunId,
        helper: &str,
        fields: BTreeMap<String, KernelValue>,
    ) -> Result<bool, MobError> {
        let run = self.require_run(run_id).await?;
        match self.evaluate_helper_value(&run.flow_state, helper, fields)? {
            KernelValue::Bool(value) => Ok(value),
            other => Err(MobError::Internal(format!(
                "flow_run helper {helper} returned non-bool value: {other:?}"
            ))),
        }
    }

    pub async fn step_branch_blocked(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.helper_bool(
            run_id,
            "StepBranchBlocked",
            BTreeMap::from([("step_id".to_string(), Self::step_id_value(step_id))]),
        )
        .await
    }

    pub async fn step_dependency_ready(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.helper_bool(
            run_id,
            "StepDependencyReady",
            BTreeMap::from([("step_id".to_string(), Self::step_id_value(step_id))]),
        )
        .await
    }

    pub async fn step_dependency_should_skip(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.helper_bool(
            run_id,
            "StepDependencyShouldSkip",
            BTreeMap::from([("step_id".to_string(), Self::step_id_value(step_id))]),
        )
        .await
    }

    pub async fn ordered_steps(&self, run_id: &RunId) -> Result<Vec<StepId>, MobError> {
        let run = self.require_run(run_id).await?;
        let seq = match run.flow_state.fields.get("ordered_steps") {
            Some(KernelValue::Seq(seq)) => seq,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run ordered_steps missing or invalid for {run_id}: {other:?}"
                )));
            }
        };
        seq.iter()
            .map(|value| match value {
                KernelValue::String(step_id) => Ok(StepId::from(step_id.clone())),
                other => Err(MobError::Internal(format!(
                    "flow_run ordered_steps entry invalid for {run_id}: {other:?}"
                ))),
            })
            .collect()
    }

    pub async fn collection_satisfied(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.helper_bool(
            run_id,
            "CollectionSatisfied",
            BTreeMap::from([("step_id".to_string(), Self::step_id_value(step_id))]),
        )
        .await
    }

    pub async fn collection_feasible(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.helper_bool(
            run_id,
            "CollectionFeasible",
            BTreeMap::from([("step_id".to_string(), Self::step_id_value(step_id))]),
        )
        .await
    }

    pub async fn register_targets(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_count: u32,
    ) -> Result<bool, MobError> {
        let run = self.require_run(run_id).await?;
        let next_state = self.transition_state(
            &run.flow_state,
            "RegisterTargets",
            BTreeMap::from([
                ("step_id".to_string(), Self::step_id_value(step_id)),
                (
                    "target_count".to_string(),
                    KernelValue::U64(u64::from(target_count)),
                ),
            ]),
        )?;
        self.run_store
            .cas_flow_state(run_id, &run.flow_state, &next_state)
            .await
    }

    pub async fn record_target_success(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<bool, MobError> {
        Ok(self
            .record_target_success_effects(run_id, step_id, target_id)
            .await?
            .is_some())
    }

    pub async fn record_target_success_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        let run = self.require_run(run_id).await?;
        let outcome = self.transition_outcome(
            &run.flow_state,
            "RecordTargetSuccess",
            BTreeMap::from([
                ("step_id".to_string(), Self::step_id_value(step_id)),
                ("target_id".to_string(), Self::target_id_value(target_id)),
            ]),
        )?;
        let transitioned = self
            .run_store
            .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
            .await?;
        if transitioned {
            Ok(Some(outcome.effects))
        } else {
            Ok(None)
        }
    }

    pub async fn record_target_terminal_failure(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<bool, MobError> {
        self.apply_step_input(run_id, "RecordTargetTerminalFailure", step_id)
            .await
    }

    pub async fn record_target_canceled(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<bool, MobError> {
        Ok(self
            .record_target_canceled_effects(run_id, step_id, target_id)
            .await?
            .is_some())
    }

    pub async fn record_target_canceled_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        let run = self.require_run(run_id).await?;
        let outcome = self.transition_outcome(
            &run.flow_state,
            "RecordTargetCanceled",
            BTreeMap::from([
                ("step_id".to_string(), Self::step_id_value(step_id)),
                ("target_id".to_string(), Self::target_id_value(target_id)),
            ]),
        )?;
        let transitioned = self
            .run_store
            .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
            .await?;
        if transitioned {
            Ok(Some(outcome.effects))
        } else {
            Ok(None)
        }
    }

    pub async fn record_target_failure(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<bool, MobError> {
        Ok(self
            .record_target_failure_effects(run_id, step_id, target_id)
            .await?
            .is_some())
    }

    pub async fn record_target_failure_effects(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        let retry_key = Self::retry_key(step_id, target_id);
        let run = self.require_run(run_id).await?;
        let outcome = self.transition_outcome(
            &run.flow_state,
            "RecordTargetFailure",
            BTreeMap::from([
                ("step_id".to_string(), Self::step_id_value(step_id)),
                ("target_id".to_string(), Self::target_id_value(target_id)),
                ("retry_key".to_string(), KernelValue::String(retry_key)),
            ]),
        )?;
        let transitioned = self
            .run_store
            .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
            .await?;
        if transitioned {
            Ok(Some(outcome.effects))
        } else {
            Ok(None)
        }
    }

    pub async fn target_retry_allowed(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        target_id: &str,
    ) -> Result<bool, MobError> {
        self.helper_bool(
            run_id,
            "TargetRetryAllowed",
            BTreeMap::from([(
                "retry_key".to_string(),
                KernelValue::String(Self::retry_key(step_id, target_id)),
            )]),
        )
        .await
    }

    pub async fn failure_count(&self, run_id: &RunId) -> Result<u32, MobError> {
        let run = self.require_run(run_id).await?;
        match run.flow_state.fields.get("failure_count") {
            Some(KernelValue::U64(value)) => u32::try_from(*value).map_err(|_| {
                MobError::Internal(format!("flow_run failure_count out of range for {run_id}"))
            }),
            other => Err(MobError::Internal(format!(
                "flow_run failure_count missing or invalid for {run_id}: {other:?}"
            ))),
        }
    }

    pub async fn step_status(
        &self,
        run_id: &RunId,
        step_id: &StepId,
    ) -> Result<Option<crate::run::StepRunStatus>, MobError> {
        let run = self.require_run(run_id).await?;
        let map = match run.flow_state.fields.get("step_status") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_status map missing or invalid for {run_id}: {other:?}"
                )));
            }
        };
        let value = map
            .get(&KernelValue::String(step_id.to_string()))
            .cloned()
            .unwrap_or(KernelValue::None);
        match value {
            KernelValue::None => Ok(None),
            value => Ok(Some(parse_step_run_status(&value, run_id)?)),
        }
    }

    pub async fn cancel_dispatched_steps(&self, run_id: &RunId) -> Result<(), MobError> {
        for step_id in self.ordered_steps(run_id).await? {
            if matches!(
                self.step_status(run_id, &step_id).await?,
                Some(crate::run::StepRunStatus::Dispatched)
            ) {
                let _ = self.cancel_step(run_id, &step_id).await?;
            }
        }
        Ok(())
    }

    pub async fn fail_dispatched_steps(&self, run_id: &RunId) -> Result<(), MobError> {
        for step_id in self.ordered_steps(run_id).await? {
            if matches!(
                self.step_status(run_id, &step_id).await?,
                Some(crate::run::StepRunStatus::Dispatched)
            ) {
                let _ = self.fail_step(run_id, &step_id).await?;
            }
        }
        Ok(())
    }

    pub async fn terminalize_completed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalize(run_id, flow_id, TerminalizationTarget::Completed)
            .await
    }

    pub async fn terminalize_failed(
        &self,
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalize(run_id, flow_id, TerminalizationTarget::Failed { reason })
            .await
    }

    pub async fn terminalize_canceled(
        &self,
        run_id: RunId,
        flow_id: FlowId,
    ) -> Result<TerminalizationOutcome, MobError> {
        self.terminalize(run_id, flow_id, TerminalizationTarget::Canceled)
            .await
    }
}

fn parse_step_run_status(
    value: &KernelValue,
    run_id: &RunId,
) -> Result<crate::run::StepRunStatus, MobError> {
    match value.as_named_variant("StepRunStatus") {
        Ok("Dispatched") => Ok(crate::run::StepRunStatus::Dispatched),
        Ok("Completed") => Ok(crate::run::StepRunStatus::Completed),
        Ok("Failed") => Ok(crate::run::StepRunStatus::Failed),
        Ok("Skipped") => Ok(crate::run::StepRunStatus::Skipped),
        Ok("Canceled") => Ok(crate::run::StepRunStatus::Canceled),
        Ok(variant) => Err(MobError::Internal(format!(
            "unknown StepRunStatus variant `{variant}` for {run_id}"
        ))),
        Err(reason) => Err(MobError::Internal(format!(
            "flow_run step_status entry invalid for {run_id}: {reason}"
        ))),
    }
}

#[allow(dead_code)] // FlowRun Phase B integration pending
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowRunEffectKind {
    AdmitStepWork,
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
            "AppendFailureLedger" => Some(Self::AppendFailureLedger),
            "EscalateSupervisor" => Some(Self::EscalateSupervisor),
            "ProjectTargetSuccess" => Some(Self::ProjectTargetSuccess),
            "ProjectTargetFailure" => Some(Self::ProjectTargetFailure),
            "ProjectTargetCanceled" => Some(Self::ProjectTargetCanceled),
            _ => None,
        }
    }
}

#[allow(dead_code)] // FlowRun Phase B integration pending
fn has_effect(effects: &[KernelEffect], expected: FlowRunEffectKind) -> bool {
    effects
        .iter()
        .filter_map(FlowRunEffectKind::parse)
        .any(|effect| effect == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MobEventKind;
    use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, MobEventStore, MobRunStore};

    #[tokio::test]
    async fn flow_run_kernel_creates_pending_runs_from_durable_truth() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-kernel"), run_store.clone(), events);
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: None,
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({"mode":"test"}))
            .await
            .expect("create pending run");
        let run = run_store
            .get_run(&run_id)
            .await
            .expect("load run")
            .expect("pending run should persist");
        assert_eq!(run.flow_id, FlowId::from("demo"));
        assert_eq!(run.status, crate::run::MobRunStatus::Pending);
        let ordered = kernel.ordered_steps(&run_id).await.expect("ordered steps");
        assert_eq!(ordered, vec![crate::ids::StepId::from("step-1")]);
    }

    #[tokio::test]
    async fn flow_run_kernel_terminalizes_without_actor_owned_fallback_state() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(
            MobId::from("mob-terminal"),
            run_store.clone(),
            events.clone(),
        );
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: None,
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };
        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");

        kernel
            .terminalize_canceled(run_id.clone(), FlowId::from("demo"))
            .await
            .expect("terminalize canceled");

        let run = run_store
            .get_run(&run_id)
            .await
            .expect("load run")
            .expect("run exists");
        assert_eq!(run.status, crate::run::MobRunStatus::Canceled);

        let replay = events.replay_all().await.expect("replay events");
        assert!(replay.iter().any(|event| matches!(
            &event.kind,
            MobEventKind::FlowCanceled { run_id: id, .. } if id == &run_id
        )));
    }

    #[tokio::test]
    async fn flow_run_kernel_tracks_collection_truth_in_machine_state() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-collection"), run_store.clone(), events);
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::Quorum { n: 2 },
                        condition: None,
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");
        let step_id = crate::ids::StepId::from("step-1");
        assert!(
            kernel
                .register_targets(&run_id, &step_id, 3)
                .await
                .expect("register targets")
        );
        assert!(
            kernel
                .collection_feasible(&run_id, &step_id)
                .await
                .expect("collection feasible before dispatch")
        );
        assert!(
            kernel
                .dispatch_step(&run_id, &step_id)
                .await
                .expect("dispatch step")
        );
        assert!(
            !kernel
                .collection_satisfied(&run_id, &step_id)
                .await
                .expect("not yet satisfied")
        );
        assert!(
            kernel
                .record_target_success(&run_id, &step_id, "worker-a")
                .await
                .expect("record first success")
        );
        assert!(
            !kernel
                .collection_satisfied(&run_id, &step_id)
                .await
                .expect("still not satisfied")
        );
        assert!(
            kernel
                .record_target_success(&run_id, &step_id, "worker-b")
                .await
                .expect("record second success")
        );
        assert!(
            kernel
                .collection_satisfied(&run_id, &step_id)
                .await
                .expect("quorum satisfied")
        );
    }

    #[tokio::test]
    async fn flow_run_kernel_tracks_all_and_any_feasibility_from_terminal_failures() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(
            MobId::from("mob-feasible"),
            run_store.clone(),
            events.clone(),
        );

        for (
            step_name,
            collection_policy,
            expected_after_one_failure,
            expected_after_all_failures,
        ) in [
            (
                "all-step",
                crate::definition::CollectionPolicy::All,
                false,
                false,
            ),
            (
                "any-step",
                crate::definition::CollectionPolicy::Any,
                true,
                false,
            ),
        ] {
            let config = FlowRunConfig {
                flow_id: FlowId::from(step_name),
                flow_spec: crate::definition::FlowSpec {
                    description: None,
                    steps: indexmap::IndexMap::from([(
                        crate::ids::StepId::from(step_name),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("do it"),
                            depends_on: Vec::new(),
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: collection_policy.clone(),
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    )]),
                },
                topology: None,
                supervisor: None,
                limits: None,
                orchestrator_role: None,
            };

            let run_id = kernel
                .create_pending_run(&config, serde_json::json!({}))
                .await
                .expect("create pending run");
            kernel.start_run(&run_id).await.expect("start run");
            let step_id = crate::ids::StepId::from(step_name);
            kernel
                .register_targets(&run_id, &step_id, 2)
                .await
                .expect("register targets");
            kernel
                .dispatch_step(&run_id, &step_id)
                .await
                .expect("dispatch step");
            kernel
                .record_target_terminal_failure(&run_id, &step_id)
                .await
                .expect("record first terminal failure");
            assert_eq!(
                kernel
                    .collection_feasible(&run_id, &step_id)
                    .await
                    .expect("check feasibility after one failure"),
                expected_after_one_failure
            );
            kernel
                .record_target_terminal_failure(&run_id, &step_id)
                .await
                .expect("record second terminal failure");
            assert_eq!(
                kernel
                    .collection_feasible(&run_id, &step_id)
                    .await
                    .expect("check feasibility after all failures"),
                expected_after_all_failures
            );
        }
    }

    #[tokio::test]
    async fn flow_run_kernel_records_condition_outcomes_explicitly() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-condition"), run_store, events);
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: Some(crate::definition::ConditionExpr::Eq {
                            path: "params.kind".to_string(),
                            value: serde_json::json!("go"),
                        }),
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");
        let step_id = crate::ids::StepId::from("step-1");

        assert!(
            kernel
                .condition_passed(&run_id, &step_id)
                .await
                .expect("record condition pass")
        );
        assert!(
            kernel
                .dispatch_step(&run_id, &step_id)
                .await
                .expect("dispatch after condition pass")
        );
        assert_eq!(
            kernel
                .step_status(&run_id, &step_id)
                .await
                .expect("step status"),
            Some(crate::run::StepRunStatus::Dispatched)
        );

        let rejected_run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create second run");
        kernel
            .start_run(&rejected_run_id)
            .await
            .expect("start second run");
        assert!(
            kernel
                .condition_rejected(&rejected_run_id, &step_id)
                .await
                .expect("record condition rejection")
        );
        assert_eq!(
            kernel
                .step_status(&rejected_run_id, &step_id)
                .await
                .expect("step status after rejection"),
            Some(crate::run::StepRunStatus::Skipped)
        );
    }

    #[tokio::test]
    async fn flow_run_kernel_refuses_dispatch_until_condition_is_recorded() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-condition-guard"), run_store, events);
        let step_id = crate::ids::StepId::from("step-1");
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    step_id.clone(),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: Some(crate::definition::ConditionExpr::Eq {
                            path: "params.kind".to_string(),
                            value: serde_json::json!("go"),
                        }),
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");

        let error = kernel
            .dispatch_step_effects(&run_id, &step_id)
            .await
            .expect_err("dispatch should be refused before condition result");
        assert!(
            error
                .to_string()
                .contains("flow_run DispatchStep transition refused"),
            "unexpected refusal: {error}"
        );
    }

    #[tokio::test]
    async fn flow_run_kernel_tracks_any_dependency_readiness_and_skip_truth() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-any-deps"), run_store, events);
        let dep_a = crate::ids::StepId::from("dep-a");
        let dep_b = crate::ids::StepId::from("dep-b");
        let gated = crate::ids::StepId::from("gated");
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([
                    (
                        dep_a.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("dep-a"),
                            depends_on: Vec::new(),
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                    (
                        dep_b.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("dep-b"),
                            depends_on: Vec::new(),
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                    (
                        gated.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("gated"),
                            depends_on: vec![dep_a.clone(), dep_b.clone()],
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: crate::definition::DependencyMode::Any,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                ]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");

        assert!(
            !kernel
                .step_dependency_ready(&run_id, &gated)
                .await
                .expect("not ready before deps")
        );
        assert!(
            !kernel
                .step_dependency_should_skip(&run_id, &gated)
                .await
                .expect("not skipped before deps settle")
        );

        kernel
            .dispatch_step(&run_id, &dep_a)
            .await
            .expect("dispatch dep-a");
        kernel
            .complete_step(&run_id, &dep_a)
            .await
            .expect("complete dep-a");

        assert!(
            kernel
                .step_dependency_ready(&run_id, &gated)
                .await
                .expect("ready after one completed dependency")
        );

        let skipped_run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create second run");
        kernel
            .start_run(&skipped_run_id)
            .await
            .expect("start second run");
        kernel
            .skip_step(&skipped_run_id, &dep_a)
            .await
            .expect("skip dep-a");
        kernel
            .skip_step(&skipped_run_id, &dep_b)
            .await
            .expect("skip dep-b");

        assert!(
            kernel
                .step_dependency_should_skip(&skipped_run_id, &gated)
                .await
                .expect("skip truth after all any-deps skipped")
        );
    }

    #[tokio::test]
    async fn flow_run_kernel_refuses_dispatch_until_dependencies_are_ready() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-dep-guard"), run_store, events);
        let dep = crate::ids::StepId::from("dep");
        let gated = crate::ids::StepId::from("gated");
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([
                    (
                        dep.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("dep"),
                            depends_on: Vec::new(),
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                    (
                        gated.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("gated"),
                            depends_on: vec![dep.clone()],
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                ]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");

        let error = kernel
            .dispatch_step_effects(&run_id, &gated)
            .await
            .expect_err("dispatch should be refused before dependencies complete");
        assert!(
            error
                .to_string()
                .contains("flow_run DispatchStep transition refused"),
            "unexpected refusal: {error}"
        );

        kernel
            .dispatch_step(&run_id, &dep)
            .await
            .expect("dispatch dependency");
        kernel
            .complete_step(&run_id, &dep)
            .await
            .expect("complete dependency");

        let effects = kernel
            .dispatch_step_effects(&run_id, &gated)
            .await
            .expect("dispatch gated step after dependency")
            .expect("dispatch should transition after dependency");
        assert!(has_effect(&effects, FlowRunEffectKind::AdmitStepWork));
    }

    #[tokio::test]
    async fn flow_run_kernel_blocks_branch_after_winner_completes() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-branch"), run_store, events);
        let first = crate::ids::StepId::from("first");
        let second = crate::ids::StepId::from("second");
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([
                    (
                        first.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("first"),
                            depends_on: Vec::new(),
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: Some(crate::ids::BranchId::from("winner")),
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                    (
                        second.clone(),
                        crate::definition::FlowStepSpec {
                            role: crate::ids::ProfileName::from("worker"),
                            message: meerkat_core::types::ContentInput::from("second"),
                            depends_on: Vec::new(),
                            dispatch_mode: crate::definition::DispatchMode::FanOut,
                            collection_policy: crate::definition::CollectionPolicy::All,
                            condition: None,
                            timeout_ms: None,
                            expected_schema_ref: None,
                            branch: Some(crate::ids::BranchId::from("winner")),
                            depends_on_mode: crate::definition::DependencyMode::All,
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: crate::definition::StepOutputFormat::Json,
                        },
                    ),
                ]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");

        assert!(
            !kernel
                .step_branch_blocked(&run_id, &second)
                .await
                .expect("branch open before winner")
        );

        kernel
            .dispatch_step(&run_id, &first)
            .await
            .expect("dispatch first");
        kernel
            .complete_step(&run_id, &first)
            .await
            .expect("complete first");

        assert!(
            kernel
                .step_branch_blocked(&run_id, &second)
                .await
                .expect("branch blocked after winner completes")
        );

        let error = kernel
            .dispatch_step_effects(&run_id, &second)
            .await
            .expect_err("dispatch should be refused after branch winner completes");
        assert!(
            error
                .to_string()
                .contains("flow_run DispatchStep transition refused"),
            "unexpected refusal: {error}"
        );
    }

    #[tokio::test]
    async fn flow_run_kernel_emits_supervisor_escalation_when_threshold_is_crossed() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-escalation"), run_store, events);
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: None,
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: Some(crate::definition::SupervisorSpec {
                role: crate::ids::ProfileName::from("supervisor"),
                escalation_threshold: 1,
            }),
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");
        let step_id = crate::ids::StepId::from("step-1");
        let dispatch_effects = kernel
            .dispatch_step_effects(&run_id, &step_id)
            .await
            .expect("dispatch effects")
            .expect("dispatch transition");
        assert!(has_effect(
            &dispatch_effects,
            FlowRunEffectKind::AdmitStepWork
        ));

        let fail_effects = kernel
            .fail_step_effects(&run_id, &step_id)
            .await
            .expect("fail effects")
            .expect("fail transition");
        assert!(has_effect(
            &fail_effects,
            FlowRunEffectKind::AppendFailureLedger
        ));
        assert!(has_effect(
            &fail_effects,
            FlowRunEffectKind::EscalateSupervisor
        ));
    }

    #[tokio::test]
    async fn flow_run_kernel_emits_target_projection_effects() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-target-effects"), run_store, events);
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: None,
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");
        let step_id = crate::ids::StepId::from("step-1");
        kernel
            .register_targets(&run_id, &step_id, 1)
            .await
            .expect("register targets");
        kernel
            .dispatch_step(&run_id, &step_id)
            .await
            .expect("dispatch step");

        let success_effects = kernel
            .record_target_success_effects(&run_id, &step_id, "worker-a")
            .await
            .expect("success effects")
            .expect("success transition");
        assert!(has_effect(
            &success_effects,
            FlowRunEffectKind::ProjectTargetSuccess
        ));

        let failure_run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create second run");
        kernel
            .start_run(&failure_run_id)
            .await
            .expect("start second run");
        kernel
            .register_targets(&failure_run_id, &step_id, 1)
            .await
            .expect("register second targets");
        kernel
            .dispatch_step(&failure_run_id, &step_id)
            .await
            .expect("dispatch second step");
        let failure_effects = kernel
            .record_target_failure_effects(&failure_run_id, &step_id, "worker-b")
            .await
            .expect("failure effects")
            .expect("failure transition");
        assert!(has_effect(
            &failure_effects,
            FlowRunEffectKind::ProjectTargetFailure
        ));
        assert!(has_effect(
            &failure_effects,
            FlowRunEffectKind::AppendFailureLedger
        ));

        let canceled_run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create third run");
        kernel
            .start_run(&canceled_run_id)
            .await
            .expect("start third run");
        kernel
            .register_targets(&canceled_run_id, &step_id, 1)
            .await
            .expect("register third targets");
        kernel
            .dispatch_step(&canceled_run_id, &step_id)
            .await
            .expect("dispatch third step");
        let canceled_effects = kernel
            .record_target_canceled_effects(&canceled_run_id, &step_id, "worker-c")
            .await
            .expect("canceled effects")
            .expect("canceled transition");
        assert!(has_effect(
            &canceled_effects,
            FlowRunEffectKind::ProjectTargetCanceled
        ));
    }

    #[tokio::test]
    async fn flow_run_kernel_rejects_target_failure_before_dispatch() {
        let run_store = Arc::new(InMemoryMobRunStore::new());
        let events = Arc::new(InMemoryMobEventStore::new());
        let kernel = FlowRunKernel::new(MobId::from("mob-target-guard"), run_store, events);
        let config = FlowRunConfig {
            flow_id: FlowId::from("demo"),
            flow_spec: crate::definition::FlowSpec {
                description: None,
                steps: indexmap::IndexMap::from([(
                    crate::ids::StepId::from("step-1"),
                    crate::definition::FlowStepSpec {
                        role: crate::ids::ProfileName::from("worker"),
                        message: meerkat_core::types::ContentInput::from("do it"),
                        depends_on: Vec::new(),
                        dispatch_mode: crate::definition::DispatchMode::FanOut,
                        collection_policy: crate::definition::CollectionPolicy::All,
                        condition: None,
                        timeout_ms: None,
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: crate::definition::DependencyMode::All,
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: crate::definition::StepOutputFormat::Json,
                    },
                )]),
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        };

        let run_id = kernel
            .create_pending_run(&config, serde_json::json!({}))
            .await
            .expect("create pending run");
        kernel.start_run(&run_id).await.expect("start run");
        let step_id = crate::ids::StepId::from("step-1");

        let error = kernel
            .record_target_failure_effects(&run_id, &step_id, "worker-a")
            .await
            .expect_err("target failure attempt before dispatch should be refused");
        assert!(
            error
                .to_string()
                .contains("flow_run RecordTargetFailure transition refused"),
            "unexpected refusal: {error}"
        );
    }
}
