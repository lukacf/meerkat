use meerkat_machine_dsl::machine;

use super::OptionValueExt;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkExecutionEvidenceKind {
    #[default]
    Completed,
    Failed,
    Canceled,
    LaunchFailed,
    RunLost,
}

machine! {
    machine WorkExecutionLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::work_execution_lifecycle",

        state {
            lifecycle_phase: WorkExecutionLifecycleState,
            binding_id: String,
            run_id: String,
            revision: u64,
            last_failure_detail: Option<String>,
            evidence_kind: Option<WorkExecutionEvidenceKind>,
        }

        init(Absent) {
            binding_id = "",
            run_id = "",
            revision = 0,
            last_failure_detail = None,
            evidence_kind = None,
        }

        terminal [FlowFailed, FlowCanceled, EvidenceProjected, WorkClosed, LaunchFailed]

        phase WorkExecutionLifecycleState {
            Absent,
            LaunchRequested,
            LaunchUncertain,
            LaunchQuarantined,
            Running,
            EvidenceProjectionRequested,
            FailureEvidenceProjectionRequested,
            CancellationEvidenceProjectionRequested,
            LaunchFailureEvidenceProjectionRequested,
            WorkClosureRequested,
            FlowFailed,
            FlowCanceled,
            EvidenceProjected,
            WorkClosed,
            LaunchFailed,
        }

        input WorkExecutionLifecycleInput {
            Bind { binding_id: String, run_id: String },
            Recover {},
            ConfirmFlowStarted {},
            ObserveFlowRunning {},
            ObserveFlowCompleted {},
            ObserveFlowFailed { detail: Option<String> },
            ObserveFlowCanceled { detail: Option<String> },
            ObserveRunLost { detail: String },
            MarkLaunchUncertain { detail: String },
            QuarantineLaunch { detail: String },
            ResolveLaunchFailed { detail: String },
            ConfirmEvidenceProjected {},
            ConfirmFlowFailureEvidenceProjected {},
            ConfirmFlowCancellationEvidenceProjected {},
            ConfirmLaunchFailureEvidenceProjected {},
            ConfirmWorkClosed {},
            RefuseWorkClosure { detail: String },
            ClassifyRetryEligibility {},
        }

        effect WorkExecutionLifecycleEffect {
            FlowLaunchRequested { binding_id: String, run_id: String },
            FlowLaunchAccepted { binding_id: String, run_id: String },
            FlowLaunchUncertain { binding_id: String, run_id: String, detail: String },
            FlowLaunchQuarantined { binding_id: String, run_id: String, detail: String },
            EvidenceProjectionRequested { binding_id: String, run_id: String, kind: WorkExecutionEvidenceKind },
            FlowFailureEvidenceProjectionRequested { binding_id: String, run_id: String, kind: WorkExecutionEvidenceKind },
            FlowCancellationEvidenceProjectionRequested { binding_id: String, run_id: String, kind: WorkExecutionEvidenceKind },
            LaunchFailureEvidenceProjectionRequested { binding_id: String, run_id: String, detail: String, kind: WorkExecutionEvidenceKind },
            WorkClosureRequested { binding_id: String },
            FlowFailed { binding_id: String, run_id: String, detail: Option<String> },
            FlowCanceled { binding_id: String, run_id: String, detail: Option<String> },
            LaunchFailed { binding_id: String, run_id: String, detail: String },
            EvidenceProjected { binding_id: String, run_id: String },
            WorkClosed { binding_id: String },
            RetryEligibilityClassified { eligible: bool },
        }

        invariant identified_after_bind {
            self.lifecycle_phase == Phase::Absent
                || (self.binding_id != "" && self.run_id != "")
        }

        invariant evidence_projection_is_typed {
            (self.lifecycle_phase != Phase::EvidenceProjectionRequested
                && self.lifecycle_phase != Phase::FailureEvidenceProjectionRequested
                && self.lifecycle_phase != Phase::CancellationEvidenceProjectionRequested
                && self.lifecycle_phase != Phase::LaunchFailureEvidenceProjectionRequested)
                || self.evidence_kind != None
        }

        disposition FlowLaunchRequested => local handoff work_execution_flow_launch seam OwnerRealizationPlusFeedback,
        disposition FlowLaunchAccepted => local handoff work_execution_flow_observation seam OwnerRealizationPlusFeedback,
        disposition FlowLaunchUncertain => local handoff work_execution_uncertain_launch_resolution seam OwnerRealizationPlusFeedback,
        disposition FlowLaunchQuarantined => local handoff work_execution_quarantined_launch_resolution seam OwnerRealizationPlusFeedback,
        disposition EvidenceProjectionRequested => local handoff work_execution_success_evidence_projection seam OwnerRealizationPlusFeedback,
        disposition FlowFailureEvidenceProjectionRequested => local handoff work_execution_failure_evidence_projection seam OwnerRealizationPlusFeedback,
        disposition FlowCancellationEvidenceProjectionRequested => local handoff work_execution_cancellation_evidence_projection seam OwnerRealizationPlusFeedback,
        disposition LaunchFailureEvidenceProjectionRequested => local handoff work_execution_launch_failure_evidence_projection seam OwnerRealizationPlusFeedback,
        disposition WorkClosureRequested => local handoff work_execution_work_closure seam OwnerRealizationPlusFeedback,
        disposition FlowFailed => local seam OwnerRealizationOnly,
        disposition FlowCanceled => local seam OwnerRealizationOnly,
        disposition LaunchFailed => local seam OwnerRealizationOnly,
        disposition EvidenceProjected => local seam OwnerRealizationOnly,
        disposition WorkClosed => local seam OwnerRealizationOnly,
        disposition RetryEligibilityClassified => local seam OwnerRealizationOnly,

        transition BindExecution {
            on input Bind { binding_id, run_id }
            guard {
                self.lifecycle_phase == Phase::Absent
                    && binding_id != ""
                    && run_id != ""
            }
            update {
                self.binding_id = binding_id;
                self.run_id = run_id;
                self.revision += 1;
                self.last_failure_detail = None;
                self.evidence_kind = None;
            }
            to LaunchRequested
            emit FlowLaunchRequested {
                binding_id: self.binding_id,
                run_id: self.run_id
            }
        }

        transition RecoverLaunchRequest {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::LaunchRequested }
            update {}
            to LaunchRequested
            emit FlowLaunchRequested { binding_id: self.binding_id, run_id: self.run_id }
        }

        transition RecoverUncertainLaunch {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::LaunchUncertain }
            update {}
            to LaunchUncertain
            emit FlowLaunchUncertain {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value")
            }
        }

        transition RecoverQuarantinedLaunch {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::LaunchQuarantined }
            update {}
            to LaunchQuarantined
            emit FlowLaunchQuarantined {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value")
            }
        }

        transition RecoverRunning {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit FlowLaunchAccepted { binding_id: self.binding_id, run_id: self.run_id }
        }

        transition RecoverEvidenceProjection {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::EvidenceProjectionRequested }
            update {}
            to EvidenceProjectionRequested
            emit EvidenceProjectionRequested { binding_id: self.binding_id, run_id: self.run_id, kind: self.evidence_kind.get("value") }
        }

        transition RecoverWorkClosure {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::WorkClosureRequested }
            update {}
            to WorkClosureRequested
            emit WorkClosureRequested { binding_id: self.binding_id }
        }

        transition RecoverFlowFailureEvidenceProjection {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::FailureEvidenceProjectionRequested }
            update {}
            to FailureEvidenceProjectionRequested
            emit FlowFailureEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition RecoverFlowCancellationEvidenceProjection {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::CancellationEvidenceProjectionRequested }
            update {}
            to CancellationEvidenceProjectionRequested
            emit FlowCancellationEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition RecoverLaunchFailureEvidenceProjection {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::LaunchFailureEvidenceProjectionRequested }
            update {}
            to LaunchFailureEvidenceProjectionRequested
            emit LaunchFailureEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value"),
                kind: self.evidence_kind.get("value")
            }
        }

        transition RecoverFlowFailure {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::FlowFailed }
            update {}
            to FlowFailed
            emit FlowFailed {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail
            }
        }

        transition RecoverFlowCancellation {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::FlowCanceled }
            update {}
            to FlowCanceled
            emit FlowCanceled {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail
            }
        }

        transition RecoverEvidenceProjected {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::EvidenceProjected }
            update {}
            to EvidenceProjected
            emit EvidenceProjected { binding_id: self.binding_id, run_id: self.run_id }
        }

        transition RecoverClosedWork {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::WorkClosed }
            update {}
            to WorkClosed
            emit WorkClosed { binding_id: self.binding_id }
        }

        transition RecoverLaunchFailure {
            on input Recover {}
            guard { self.lifecycle_phase == Phase::LaunchFailed }
            update {}
            to LaunchFailed
            emit LaunchFailed {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value")
            }
        }

        transition AcceptFlowLaunch {
            on input ConfirmFlowStarted {}
            guard {
                self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
                    || self.lifecycle_phase == Phase::LaunchQuarantined
            }
            update {
                self.revision += 1;
                self.last_failure_detail = None;
                self.evidence_kind = None;
            }
            to Running
            emit FlowLaunchAccepted {
                binding_id: self.binding_id,
                run_id: self.run_id
            }
        }

        transition ObserveRunningFlow {
            on input ObserveFlowRunning {}
            guard {
                self.lifecycle_phase == Phase::Running
                    || self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
                    || self.lifecycle_phase == Phase::LaunchQuarantined
            }
            update {
                self.revision += 1;
                self.last_failure_detail = None;
                self.evidence_kind = None;
            }
            to Running
            emit FlowLaunchAccepted {
                binding_id: self.binding_id,
                run_id: self.run_id
            }
        }

        transition ObserveCompletedFlow {
            on input ObserveFlowCompleted {}
            guard {
                self.lifecycle_phase == Phase::Running
                    || self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
                    || self.lifecycle_phase == Phase::LaunchQuarantined
            }
            update {
                self.revision += 1;
                self.last_failure_detail = None;
                self.evidence_kind = Some(WorkExecutionEvidenceKind::Completed);
            }
            to EvidenceProjectionRequested
            emit EvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition ObserveFailedFlow {
            on input ObserveFlowFailed { detail }
            guard {
                self.lifecycle_phase == Phase::Running
                    || self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
                    || self.lifecycle_phase == Phase::LaunchQuarantined
            }
            update {
                self.revision += 1;
                self.last_failure_detail = detail;
                self.evidence_kind = Some(WorkExecutionEvidenceKind::Failed);
            }
            to FailureEvidenceProjectionRequested
            emit FlowFailureEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition ObserveCanceledFlow {
            on input ObserveFlowCanceled { detail }
            guard {
                self.lifecycle_phase == Phase::Running
                    || self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
                    || self.lifecycle_phase == Phase::LaunchQuarantined
            }
            update {
                self.revision += 1;
                self.last_failure_detail = detail;
                self.evidence_kind = Some(WorkExecutionEvidenceKind::Canceled);
            }
            to CancellationEvidenceProjectionRequested
            emit FlowCancellationEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition ObserveLostRun {
            on input ObserveRunLost { detail }
            guard {
                self.lifecycle_phase == Phase::Running
            }
            update {
                self.revision += 1;
                self.last_failure_detail = Some(detail);
                self.evidence_kind = Some(WorkExecutionEvidenceKind::RunLost);
            }
            to FailureEvidenceProjectionRequested
            emit FlowFailureEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition ObserveLostCompletedRunBeforeEvidence {
            on input ObserveRunLost { detail }
            guard {
                self.lifecycle_phase == Phase::EvidenceProjectionRequested
            }
            update {
                self.revision += 1;
                self.last_failure_detail = Some(detail);
                self.evidence_kind = Some(WorkExecutionEvidenceKind::RunLost);
            }
            to FailureEvidenceProjectionRequested
            emit FlowFailureEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                kind: self.evidence_kind.get("value")
            }
        }

        transition RecordUncertainLaunch {
            on input MarkLaunchUncertain { detail }
            guard {
                self.lifecycle_phase == Phase::LaunchRequested
            }
            update {
                self.revision += 1;
                self.last_failure_detail = Some(detail);
                self.evidence_kind = None;
            }
            to LaunchUncertain
            emit FlowLaunchUncertain {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value")
            }
        }

        transition QuarantineLaunch {
            on input QuarantineLaunch { detail }
            guard {
                self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
            }
            update {
                self.revision += 1;
                self.last_failure_detail = Some(detail);
                self.evidence_kind = None;
            }
            to LaunchQuarantined
            emit FlowLaunchQuarantined {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value")
            }
        }

        transition FailLaunch {
            on input ResolveLaunchFailed { detail }
            guard {
                self.lifecycle_phase == Phase::LaunchRequested
                    || self.lifecycle_phase == Phase::LaunchUncertain
            }
            update {
                self.revision += 1;
                self.last_failure_detail = Some(detail);
                self.evidence_kind = Some(WorkExecutionEvidenceKind::LaunchFailed);
            }
            to LaunchFailureEvidenceProjectionRequested
            emit LaunchFailureEvidenceProjectionRequested {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value"),
                kind: self.evidence_kind.get("value")
            }
        }

        transition CommitLaunchFailureEvidenceProjection {
            on input ConfirmLaunchFailureEvidenceProjected {}
            guard {
                self.lifecycle_phase == Phase::LaunchFailureEvidenceProjectionRequested
            }
            update {
                self.revision += 1;
            }
            to LaunchFailed
            emit LaunchFailed {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail.get("value")
            }
        }

        transition CommitEvidenceProjection {
            on input ConfirmEvidenceProjected {}
            guard {
                self.lifecycle_phase == Phase::EvidenceProjectionRequested
            }
            update {
                self.revision += 1;
            }
            to WorkClosureRequested
            emit WorkClosureRequested { binding_id: self.binding_id }
        }

        transition CommitFlowFailureEvidenceProjection {
            on input ConfirmFlowFailureEvidenceProjected {}
            guard {
                self.lifecycle_phase == Phase::FailureEvidenceProjectionRequested
            }
            update {
                self.revision += 1;
            }
            to FlowFailed
            emit FlowFailed {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail
            }
        }

        transition CommitFlowCancellationEvidenceProjection {
            on input ConfirmFlowCancellationEvidenceProjected {}
            guard {
                self.lifecycle_phase == Phase::CancellationEvidenceProjectionRequested
            }
            update {
                self.revision += 1;
            }
            to FlowCanceled
            emit FlowCanceled {
                binding_id: self.binding_id,
                run_id: self.run_id,
                detail: self.last_failure_detail
            }
        }

        transition CommitWorkClosure {
            on input ConfirmWorkClosed {}
            guard {
                self.lifecycle_phase == Phase::WorkClosureRequested
            }
            update {
                self.revision += 1;
                self.last_failure_detail = None;
            }
            to WorkClosed
            emit WorkClosed { binding_id: self.binding_id }
        }

        transition RecordWorkClosureRefusal {
            on input RefuseWorkClosure { detail }
            guard {
                self.lifecycle_phase == Phase::WorkClosureRequested
            }
            update {
                self.revision += 1;
                self.last_failure_detail = Some(detail);
            }
            to EvidenceProjected
            emit EvidenceProjected {
                binding_id: self.binding_id,
                run_id: self.run_id
            }
        }

        // The retry/supersession verdict is a machine-owned classification of
        // the declared terminal phase set. Each generated transition self-loops
        // in the recovered phase and emits only the typed verdict.
        transition ClassifyRetryEligibilityTerminal {
            per_phase [FlowFailed, FlowCanceled, EvidenceProjected, WorkClosed, LaunchFailed]
            on input ClassifyRetryEligibility {}
            update {}
            to Absent
            emit RetryEligibilityClassified { eligible: true }
        }

        transition ClassifyRetryEligibilityLive {
            per_phase [Absent, LaunchRequested, LaunchUncertain, LaunchQuarantined, Running, EvidenceProjectionRequested, FailureEvidenceProjectionRequested, CancellationEvidenceProjectionRequested, LaunchFailureEvidenceProjectionRequested, WorkClosureRequested]
            on input ClassifyRetryEligibility {}
            update {}
            to Absent
            emit RetryEligibilityClassified { eligible: false }
        }
    }
}

impl serde::Serialize for WorkExecutionLifecycleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Absent => "absent",
            Self::LaunchRequested => "launch_requested",
            Self::LaunchUncertain => "launch_uncertain",
            Self::LaunchQuarantined => "launch_quarantined",
            Self::Running => "running",
            Self::EvidenceProjectionRequested => "evidence_projection_requested",
            Self::FailureEvidenceProjectionRequested => "failure_evidence_projection_requested",
            Self::CancellationEvidenceProjectionRequested => {
                "cancellation_evidence_projection_requested"
            }
            Self::LaunchFailureEvidenceProjectionRequested => {
                "launch_failure_evidence_projection_requested"
            }
            Self::WorkClosureRequested => "work_closure_requested",
            Self::FlowFailed => "flow_failed",
            Self::FlowCanceled => "flow_canceled",
            Self::EvidenceProjected => "evidence_projected",
            Self::WorkClosed => "work_closed",
            Self::LaunchFailed => "launch_failed",
        })
    }
}

impl<'de> serde::Deserialize<'de> for WorkExecutionLifecycleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "absent" => Ok(Self::Absent),
            "launch_requested" => Ok(Self::LaunchRequested),
            "launch_uncertain" => Ok(Self::LaunchUncertain),
            "launch_quarantined" => Ok(Self::LaunchQuarantined),
            "running" => Ok(Self::Running),
            "evidence_projection_requested" => Ok(Self::EvidenceProjectionRequested),
            "failure_evidence_projection_requested" => Ok(Self::FailureEvidenceProjectionRequested),
            "cancellation_evidence_projection_requested" => {
                Ok(Self::CancellationEvidenceProjectionRequested)
            }
            "launch_failure_evidence_projection_requested" => {
                Ok(Self::LaunchFailureEvidenceProjectionRequested)
            }
            "work_closure_requested" => Ok(Self::WorkClosureRequested),
            "flow_failed" => Ok(Self::FlowFailed),
            "flow_canceled" => Ok(Self::FlowCanceled),
            "evidence_projected" => Ok(Self::EvidenceProjected),
            "work_closed" => Ok(Self::WorkClosed),
            "launch_failed" => Ok(Self::LaunchFailed),
            other => Err(serde::de::Error::custom(format!(
                "invalid WorkExecutionLifecycleState `{other}`"
            ))),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WorkExecutionLifecycleMachineStateWire {
    lifecycle_phase: WorkExecutionLifecycleState,
    binding_id: String,
    run_id: String,
    revision: u64,
    #[serde(default)]
    last_failure_detail: Option<String>,
    #[serde(default)]
    evidence_kind: Option<WorkExecutionEvidenceKind>,
}

impl From<&WorkExecutionLifecycleMachineState> for WorkExecutionLifecycleMachineStateWire {
    fn from(state: &WorkExecutionLifecycleMachineState) -> Self {
        Self {
            lifecycle_phase: state.lifecycle_phase,
            binding_id: state.binding_id.clone(),
            run_id: state.run_id.clone(),
            revision: state.revision,
            last_failure_detail: state.last_failure_detail.clone(),
            evidence_kind: state.evidence_kind,
        }
    }
}

impl From<WorkExecutionLifecycleMachineStateWire> for WorkExecutionLifecycleMachineState {
    fn from(wire: WorkExecutionLifecycleMachineStateWire) -> Self {
        Self {
            lifecycle_phase: wire.lifecycle_phase,
            binding_id: wire.binding_id,
            run_id: wire.run_id,
            revision: wire.revision,
            last_failure_detail: wire.last_failure_detail,
            evidence_kind: wire.evidence_kind,
        }
    }
}

impl serde::Serialize for WorkExecutionLifecycleMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        WorkExecutionLifecycleMachineStateWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for WorkExecutionLifecycleMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        WorkExecutionLifecycleMachineStateWire::deserialize(deserializer).map(Self::from)
    }
}
