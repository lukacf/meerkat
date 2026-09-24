use crate::WorkGraphError;
use crate::generated::{
    protocol_work_execution_cancellation_evidence_projection as cancellation_evidence_protocol,
    protocol_work_execution_failure_evidence_projection as failure_evidence_protocol,
    protocol_work_execution_flow_launch as flow_launch_protocol,
    protocol_work_execution_flow_observation as flow_observation_protocol,
    protocol_work_execution_launch_failure_evidence_projection as launch_failure_evidence_protocol,
    protocol_work_execution_quarantined_launch_resolution as quarantined_launch_protocol,
    protocol_work_execution_success_evidence_projection as success_evidence_protocol,
    protocol_work_execution_uncertain_launch_resolution as uncertain_launch_protocol,
    protocol_work_execution_work_closure as work_closure_protocol,
};
use crate::machines::work_execution_lifecycle as execution_dsl;
use crate::types::{WorkExecutionBinding, WorkExecutionBindingId, WorkExecutionMachineState};

pub use execution_dsl::{WorkExecutionLifecycleEffect, WorkExecutionLifecycleState};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkExecutionObservation {
    FlowStarted,
    FlowRunning,
    FlowCompleted,
    FlowFailed { detail: Option<String> },
    FlowCanceled { detail: Option<String> },
    FlowRunLost { detail: String },
    LaunchUncertain { detail: String },
    LaunchQuarantined { detail: String },
    LaunchFailed { detail: String },
    EvidenceProjected,
    FlowFailureEvidenceProjected,
    FlowCancellationEvidenceProjected,
    LaunchFailureEvidenceProjected,
    WorkClosed,
    WorkClosureRefused { detail: String },
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WorkExecutionMachine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkExecutionTransition {
    pub binding: WorkExecutionBinding,
    pub effect: WorkExecutionLifecycleEffect,
}

/// Sealed machine-minted authority for the initial durable binding commit.
pub struct WorkExecutionBindCommit {
    binding: WorkExecutionBinding,
    effect: WorkExecutionLifecycleEffect,
}

impl WorkExecutionBindCommit {
    pub(crate) fn into_parts(self) -> (WorkExecutionBinding, WorkExecutionLifecycleEffect) {
        (self.binding, self.effect)
    }

    pub(crate) fn binding(&self) -> &WorkExecutionBinding {
        &self.binding
    }

    pub(crate) fn effect(&self) -> &WorkExecutionLifecycleEffect {
        &self.effect
    }
}

/// Sealed machine-minted authority for one durable observation transition.
pub struct WorkExecutionObservationCommit {
    previous: WorkExecutionBinding,
    observation: WorkExecutionObservation,
    binding: WorkExecutionBinding,
    effect: WorkExecutionLifecycleEffect,
}

impl WorkExecutionObservationCommit {
    pub(crate) fn into_parts(
        self,
    ) -> (
        WorkExecutionBinding,
        WorkExecutionObservation,
        WorkExecutionBinding,
        WorkExecutionLifecycleEffect,
    ) {
        (self.previous, self.observation, self.binding, self.effect)
    }

    pub(crate) fn binding(&self) -> &WorkExecutionBinding {
        &self.binding
    }

    pub(crate) fn effect(&self) -> &WorkExecutionLifecycleEffect {
        &self.effect
    }
}

impl WorkExecutionMachine {
    pub(crate) fn prepare_bind(
        binding: WorkExecutionBinding,
    ) -> Result<WorkExecutionBindCommit, WorkGraphError> {
        binding.validate()?;
        let (expected, effect) = Self::bind(&binding.binding_id, binding.target.run_id())?;
        if binding.machine_state != expected {
            return Err(WorkGraphError::InvalidInput(format!(
                "work execution binding {} was not initialized by WorkExecutionLifecycleMachine",
                binding.binding_id
            )));
        }
        Ok(WorkExecutionBindCommit { binding, effect })
    }

    pub(crate) fn prepare_observation(
        previous: WorkExecutionBinding,
        expected_revision: u64,
        observation: WorkExecutionObservation,
    ) -> Result<WorkExecutionObservationCommit, WorkGraphError> {
        let (binding, effect) =
            Self::observe(previous.clone(), expected_revision, observation.clone())?;
        Ok(WorkExecutionObservationCommit {
            previous,
            observation,
            binding,
            effect,
        })
    }
    pub fn recover_effect(
        binding: &WorkExecutionBinding,
    ) -> Result<WorkExecutionLifecycleEffect, WorkGraphError> {
        validate_projection(binding)?;
        let mut authority =
            execution_dsl::WorkExecutionLifecycleMachineAuthority::recover_from_state(
                binding.machine_state.clone(),
            )
            .map_err(|error| {
                WorkGraphError::InvalidTransition(format!(
                    "work execution {} refused recovery: {error:?}",
                    binding.binding_id
                ))
            })?;
        let transition = execution_dsl::WorkExecutionLifecycleMachineMutator::apply(
            &mut authority,
            execution_dsl::WorkExecutionLifecycleInput::Recover {},
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work execution {} refused effect recovery: {error:?}",
                binding.binding_id
            ))
        })?;
        validate_handoff_obligation(&transition)?;
        exactly_one_effect(transition.effects())
    }

    pub fn bind(
        binding_id: &WorkExecutionBindingId,
        run_id: &str,
    ) -> Result<(WorkExecutionMachineState, WorkExecutionLifecycleEffect), WorkGraphError> {
        let mut authority = execution_dsl::WorkExecutionLifecycleMachineAuthority::new();
        let transition = execution_dsl::WorkExecutionLifecycleMachineMutator::apply(
            &mut authority,
            execution_dsl::WorkExecutionLifecycleInput::Bind {
                binding_id: binding_id.as_str().to_string(),
                run_id: run_id.to_string(),
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "generated work execution binding transition refused: {error:?}"
            ))
        })?;
        validate_handoff_obligation(&transition)?;
        let effect = exactly_one_effect(transition.effects())?;
        Ok((authority.state().clone(), effect))
    }

    pub fn observe(
        mut binding: WorkExecutionBinding,
        expected_revision: u64,
        observation: WorkExecutionObservation,
    ) -> Result<(WorkExecutionBinding, WorkExecutionLifecycleEffect), WorkGraphError> {
        validate_observation_detail(&observation)?;
        if binding.machine_state.revision != expected_revision {
            return Err(WorkGraphError::Conflict(format!(
                "stale work execution revision for {}: expected {}, actual {}",
                binding.binding_id, expected_revision, binding.machine_state.revision
            )));
        }
        validate_projection(&binding)?;
        let mut authority =
            execution_dsl::WorkExecutionLifecycleMachineAuthority::recover_from_state(
                binding.machine_state.clone(),
            )
            .map_err(|error| {
                WorkGraphError::InvalidTransition(format!(
                    "work execution {} refused recovery: {error:?}",
                    binding.binding_id
                ))
            })?;
        let mut recovery_authority =
            execution_dsl::WorkExecutionLifecycleMachineAuthority::recover_from_state(
                binding.machine_state.clone(),
            )
            .map_err(|error| {
                WorkGraphError::InvalidTransition(format!(
                    "work execution {} refused handoff recovery: {error:?}",
                    binding.binding_id
                ))
            })?;
        let recovery_transition = execution_dsl::WorkExecutionLifecycleMachineMutator::apply(
            &mut recovery_authority,
            execution_dsl::WorkExecutionLifecycleInput::Recover {},
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work execution {} refused handoff effect recovery: {error:?}",
                binding.binding_id
            ))
        })?;
        validate_handoff_obligation(&recovery_transition)?;
        let recovered_effect = exactly_one_effect(recovery_transition.effects())?;
        let observation_debug = format!("{observation:?}");
        macro_rules! submit {
            ($protocol:ident, $function:ident $(, $argument:expr)*) => {{
                let obligation = exactly_one_obligation(
                    $protocol::extract_obligations(&recovery_transition),
                    &binding.binding_id,
                )?;
                $protocol::$function(&mut authority, obligation $(, $argument)*)
            }};
        }
        let transition = match (&recovered_effect, observation) {
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::FlowStarted) =>
                submit!(flow_launch_protocol, submit_confirm_flow_started),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::FlowRunning) =>
                submit!(flow_launch_protocol, submit_observe_flow_running),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::FlowCompleted) =>
                submit!(flow_launch_protocol, submit_observe_flow_completed),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::FlowFailed { detail }) =>
                submit!(flow_launch_protocol, submit_observe_flow_failed, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::FlowCanceled { detail }) =>
                submit!(flow_launch_protocol, submit_observe_flow_canceled, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::LaunchUncertain { detail }) =>
                submit!(flow_launch_protocol, submit_mark_launch_uncertain, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::LaunchQuarantined { detail }) =>
                submit!(flow_launch_protocol, submit_quarantine_launch, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }, WorkExecutionObservation::LaunchFailed { detail }) =>
                submit!(flow_launch_protocol, submit_resolve_launch_failed, detail),

            (WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }, WorkExecutionObservation::FlowRunning) =>
                submit!(flow_observation_protocol, submit_observe_flow_running),
            (WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }, WorkExecutionObservation::FlowCompleted) =>
                submit!(flow_observation_protocol, submit_observe_flow_completed),
            (WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }, WorkExecutionObservation::FlowFailed { detail }) =>
                submit!(flow_observation_protocol, submit_observe_flow_failed, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }, WorkExecutionObservation::FlowCanceled { detail }) =>
                submit!(flow_observation_protocol, submit_observe_flow_canceled, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }, WorkExecutionObservation::FlowRunLost { detail }) =>
                submit!(flow_observation_protocol, submit_observe_run_lost, detail),

            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::FlowStarted) =>
                submit!(uncertain_launch_protocol, submit_confirm_flow_started),
            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::FlowRunning) =>
                submit!(uncertain_launch_protocol, submit_observe_flow_running),
            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::FlowCompleted) =>
                submit!(uncertain_launch_protocol, submit_observe_flow_completed),
            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::FlowFailed { detail }) =>
                submit!(uncertain_launch_protocol, submit_observe_flow_failed, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::FlowCanceled { detail }) =>
                submit!(uncertain_launch_protocol, submit_observe_flow_canceled, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::LaunchFailed { detail }) =>
                submit!(uncertain_launch_protocol, submit_resolve_launch_failed, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }, WorkExecutionObservation::LaunchQuarantined { detail }) =>
                submit!(uncertain_launch_protocol, submit_quarantine_launch, detail),

            (WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }, WorkExecutionObservation::FlowStarted) =>
                submit!(quarantined_launch_protocol, submit_confirm_flow_started),
            (WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }, WorkExecutionObservation::FlowRunning) =>
                submit!(quarantined_launch_protocol, submit_observe_flow_running),
            (WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }, WorkExecutionObservation::FlowCompleted) =>
                submit!(quarantined_launch_protocol, submit_observe_flow_completed),
            (WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }, WorkExecutionObservation::FlowFailed { detail }) =>
                submit!(quarantined_launch_protocol, submit_observe_flow_failed, detail),
            (WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }, WorkExecutionObservation::FlowCanceled { detail }) =>
                submit!(quarantined_launch_protocol, submit_observe_flow_canceled, detail),

            (WorkExecutionLifecycleEffect::EvidenceProjectionRequested { .. }, WorkExecutionObservation::EvidenceProjected) =>
                submit!(success_evidence_protocol, submit_confirm_evidence_projected),
            (WorkExecutionLifecycleEffect::EvidenceProjectionRequested { .. }, WorkExecutionObservation::FlowRunLost { detail }) =>
                submit!(success_evidence_protocol, submit_observe_run_lost, detail),
            (WorkExecutionLifecycleEffect::FlowFailureEvidenceProjectionRequested { .. }, WorkExecutionObservation::FlowFailureEvidenceProjected) =>
                submit!(failure_evidence_protocol, submit_confirm_flow_failure_evidence_projected),
            (WorkExecutionLifecycleEffect::FlowCancellationEvidenceProjectionRequested { .. }, WorkExecutionObservation::FlowCancellationEvidenceProjected) =>
                submit!(cancellation_evidence_protocol, submit_confirm_flow_cancellation_evidence_projected),
            (WorkExecutionLifecycleEffect::LaunchFailureEvidenceProjectionRequested { .. }, WorkExecutionObservation::LaunchFailureEvidenceProjected) =>
                submit!(launch_failure_evidence_protocol, submit_confirm_launch_failure_evidence_projected),
            (WorkExecutionLifecycleEffect::WorkClosureRequested { .. }, WorkExecutionObservation::WorkClosed) =>
                submit!(work_closure_protocol, submit_confirm_work_closed),
            (WorkExecutionLifecycleEffect::WorkClosureRequested { .. }, WorkExecutionObservation::WorkClosureRefused { detail }) =>
                submit!(work_closure_protocol, submit_refuse_work_closure, detail),
            _ => {
                return Err(WorkGraphError::InvalidTransition(format!(
                    "work execution {} observation {observation_debug} is not admitted by the generated owner-feedback protocol for {recovered_effect:?}",
                    binding.binding_id
                )));
            }
        }
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work execution {} refused observation: {error:?}",
                binding.binding_id
            ))
        })?;
        validate_handoff_obligation(&transition)?;
        let effect = exactly_one_effect(transition.effects())?;
        binding.machine_state = authority.state().clone();
        validate_projection(&binding)?;
        Ok((binding, effect))
    }

    pub fn validate_projection(binding: &WorkExecutionBinding) -> Result<(), WorkGraphError> {
        validate_projection(binding)
    }

    /// The lifecycle authority's single retry/supersession classifier.
    pub fn retry_eligible(binding: &WorkExecutionBinding) -> Result<bool, WorkGraphError> {
        validate_projection(binding)?;
        let mut authority =
            execution_dsl::WorkExecutionLifecycleMachineAuthority::recover_from_state(
                binding.machine_state.clone(),
            )
            .map_err(|error| {
                WorkGraphError::InvalidTransition(format!(
                    "work execution {} refused retry classification recovery: {error:?}",
                    binding.binding_id
                ))
            })?;
        let transition = execution_dsl::WorkExecutionLifecycleMachineMutator::apply(
            &mut authority,
            execution_dsl::WorkExecutionLifecycleInput::ClassifyRetryEligibility {},
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work execution {} refused retry classification: {error:?}",
                binding.binding_id
            ))
        })?;
        match exactly_one_effect(transition.effects())? {
            WorkExecutionLifecycleEffect::RetryEligibilityClassified { eligible } => Ok(eligible),
            effect => Err(WorkGraphError::Store(format!(
                "work execution {} retry classifier emitted unexpected effect {effect:?}",
                binding.binding_id
            ))),
        }
    }
}

fn validate_observation_detail(
    observation: &WorkExecutionObservation,
) -> Result<(), WorkGraphError> {
    const MAX_DETAIL_BYTES: usize = 4096;
    let detail = match observation {
        WorkExecutionObservation::FlowFailed { detail }
        | WorkExecutionObservation::FlowCanceled { detail } => detail.as_deref(),
        WorkExecutionObservation::FlowRunLost { detail }
        | WorkExecutionObservation::LaunchUncertain { detail }
        | WorkExecutionObservation::LaunchQuarantined { detail }
        | WorkExecutionObservation::LaunchFailed { detail }
        | WorkExecutionObservation::WorkClosureRefused { detail } => Some(detail.as_str()),
        _ => None,
    };
    if let Some(detail) = detail
        && (detail.len() > MAX_DETAIL_BYTES || detail.chars().any(char::is_control))
    {
        return Err(WorkGraphError::InvalidInput(format!(
            "work execution observation detail must be single-line text no longer than {MAX_DETAIL_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_handoff_obligation(
    transition: &execution_dsl::WorkExecutionLifecycleMachineTransition,
) -> Result<(), WorkGraphError> {
    let obligation_count = match transition.effects() {
        [WorkExecutionLifecycleEffect::FlowLaunchRequested { .. }] => {
            flow_launch_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }] => {
            flow_observation_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }] => {
            uncertain_launch_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }] => {
            quarantined_launch_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::EvidenceProjectionRequested { .. }] => {
            success_evidence_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::FlowFailureEvidenceProjectionRequested { .. }] => {
            failure_evidence_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::FlowCancellationEvidenceProjectionRequested { .. }] => {
            cancellation_evidence_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::LaunchFailureEvidenceProjectionRequested { .. }] => {
            launch_failure_evidence_protocol::extract_obligations(transition).len()
        }
        [WorkExecutionLifecycleEffect::WorkClosureRequested { .. }] => {
            work_closure_protocol::extract_obligations(transition).len()
        }
        _ => return Ok(()),
    };
    if obligation_count != 1 {
        return Err(WorkGraphError::Store(format!(
            "generated work execution transition projected {obligation_count} handoff obligations, expected exactly one"
        )));
    }
    Ok(())
}

fn validate_projection(binding: &WorkExecutionBinding) -> Result<(), WorkGraphError> {
    if binding.machine_state.binding_id != binding.binding_id.as_str() {
        return Err(WorkGraphError::Store(format!(
            "work execution {} machine binding identity does not match its projection",
            binding.binding_id
        )));
    }
    if binding.machine_state.run_id != binding.target.run_id() {
        return Err(WorkGraphError::Store(format!(
            "work execution {} machine run identity does not match its target",
            binding.binding_id
        )));
    }
    Ok(())
}

fn exactly_one_effect(
    effects: &[WorkExecutionLifecycleEffect],
) -> Result<WorkExecutionLifecycleEffect, WorkGraphError> {
    match effects {
        [effect] => Ok(effect.clone()),
        _ => Err(WorkGraphError::Store(format!(
            "generated work execution transition emitted {} effects, expected exactly one",
            effects.len()
        ))),
    }
}

fn exactly_one_obligation<T>(
    obligations: Vec<T>,
    binding_id: &WorkExecutionBindingId,
) -> Result<T, WorkGraphError> {
    let count = obligations.len();
    obligations.into_iter().next().filter(|_| count == 1).ok_or_else(|| {
        WorkGraphError::Store(format!(
            "generated work execution handoff for {binding_id} projected {count} obligations, expected exactly one"
        ))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{WorkExecutionTarget, WorkItemId, WorkItemRef, WorkNamespace};
    use chrono::Utc;
    use serde_json::json;

    fn binding() -> WorkExecutionBinding {
        let binding_id = WorkExecutionBindingId::new("execution-machine-test").expect("id");
        let target = WorkExecutionTarget::mob_flow(
            "mob",
            "flow",
            format!("sha256:{}", "c".repeat(64)),
            "1ae92ab4-8afe-5ad2-b9c3-fccae4f569a5",
            crate::WorkExecutionAuthority::TargetOwner,
            json!({}),
        )
        .expect("target");
        let (machine_state, _) =
            WorkExecutionMachine::bind(&binding_id, target.run_id()).expect("bind");
        WorkExecutionBinding {
            binding_id,
            work_ref: WorkItemRef {
                realm_id: "realm".to_string(),
                namespace: WorkNamespace::default(),
                item_id: WorkItemId::new("item").expect("item"),
            },
            target,
            idempotency_key: "key".to_string(),
            correlation_id: "229650c5-9372-53e9-9c3a-831638a47c77".to_string(),
            supersedes: None,
            machine_state,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn failed_flow_requires_evidence_projection_before_terminal_attempt() {
        let binding = binding();
        let (running, _) =
            WorkExecutionMachine::observe(binding, 1, WorkExecutionObservation::FlowRunning)
                .expect("running");
        let (projecting, effect) = WorkExecutionMachine::observe(
            running,
            2,
            WorkExecutionObservation::FlowFailed {
                detail: Some("step failed".to_string()),
            },
        )
        .expect("failure observed");
        assert!(matches!(
            effect,
            WorkExecutionLifecycleEffect::FlowFailureEvidenceProjectionRequested { .. }
        ));
        let (terminal, effect) = WorkExecutionMachine::observe(
            projecting,
            3,
            WorkExecutionObservation::FlowFailureEvidenceProjected,
        )
        .expect("failure evidence projected");
        assert!(matches!(
            effect,
            WorkExecutionLifecycleEffect::FlowFailed { .. }
        ));
        assert!(matches!(
            WorkExecutionMachine::recover_effect(&terminal).expect("recover terminal"),
            WorkExecutionLifecycleEffect::FlowFailed { .. }
        ));
    }

    #[test]
    fn uncertain_launch_recovers_as_uncertain_without_redrive_request() {
        let binding = binding();
        let (uncertain, effect) = WorkExecutionMachine::observe(
            binding,
            1,
            WorkExecutionObservation::LaunchUncertain {
                detail: "intent exists without run".to_string(),
            },
        )
        .expect("uncertain");
        assert!(matches!(
            effect,
            WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }
        ));
        assert!(matches!(
            WorkExecutionMachine::recover_effect(&uncertain).expect("recover uncertain"),
            WorkExecutionLifecycleEffect::FlowLaunchUncertain { .. }
        ));
    }

    #[test]
    fn observations_are_admitted_only_by_the_current_generated_handoff() {
        let binding = binding();
        let (accepted, _) =
            WorkExecutionMachine::observe(binding, 1, WorkExecutionObservation::FlowStarted)
                .expect("launch accepted");
        let error =
            WorkExecutionMachine::observe(accepted, 2, WorkExecutionObservation::FlowStarted)
                .expect_err("flow-observation protocol does not admit a second start");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }

    #[test]
    fn quarantined_launch_accepts_only_observed_exact_run_feedback() {
        let binding = binding();
        let (quarantined, effect) = WorkExecutionMachine::observe(
            binding,
            1,
            WorkExecutionObservation::LaunchQuarantined {
                detail: "realizing ledger has no exact run".to_string(),
            },
        )
        .expect("quarantine");
        assert!(matches!(
            effect,
            WorkExecutionLifecycleEffect::FlowLaunchQuarantined { .. }
        ));
        assert!(
            !WorkExecutionMachine::retry_eligible(&quarantined)
                .expect("classify quarantined launch"),
            "quarantine may still have a live realizer and cannot authorize supersession"
        );
        let (_, effect) =
            WorkExecutionMachine::observe(quarantined, 2, WorkExecutionObservation::FlowStarted)
                .expect("exact run observation feedback");
        assert!(matches!(
            effect,
            WorkExecutionLifecycleEffect::FlowLaunchAccepted { .. }
        ));
    }
}
