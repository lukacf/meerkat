//! MobMachine-owned flow-run projection reducer.
//!
//! This module is intentionally not exported through generated kernel modules.
//! It is the runtime projection shape used by `MobRun` while the canonical
//! semantic owner remains `machines::mob_machine`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::implicit_clone,
    clippy::unnecessary_cast,
    clippy::redundant_clone
)]

pub use crate::ids::MeerkatId;
pub use crate::ids::{BranchId, FrameId, LoopInstanceId, StepId};

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CollectionPolicyKind {
    All,
    Any,
    Quorum,
}
impl CollectionPolicyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Any => "Any",
            Self::Quorum => "Quorum",
        }
    }
}
impl std::fmt::Display for CollectionPolicyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DependencyMode {
    All,
    Any,
}
impl DependencyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Any => "Any",
        }
    }
}
impl std::fmt::Display for DependencyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FlowRunStatus {
    Absent,
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}
impl FlowRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Absent => "Absent",
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Canceled => "Canceled",
        }
    }
}
impl std::fmt::Display for FlowRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StepRunStatus {
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}
impl StepRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dispatched => "Dispatched",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
            Self::Canceled => "Canceled",
        }
    }
}
impl std::fmt::Display for StepRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Context {}
pub struct EmptyContext;
impl Context for EmptyContext {}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Absent,
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub phase: Phase,
    pub tracked_steps: std::collections::BTreeSet<StepId>,
    pub ordered_steps: Vec<StepId>,
    pub step_status: std::collections::BTreeMap<StepId, Option<StepRunStatus>>,
    pub output_recorded: std::collections::BTreeMap<StepId, bool>,
    pub step_condition_results: std::collections::BTreeMap<StepId, Option<bool>>,
    pub step_has_conditions: std::collections::BTreeMap<StepId, bool>,
    pub step_dependencies: std::collections::BTreeMap<StepId, Vec<StepId>>,
    pub step_dependency_modes: std::collections::BTreeMap<StepId, DependencyMode>,
    pub step_branches: std::collections::BTreeMap<StepId, Option<BranchId>>,
    pub step_collection_policies: std::collections::BTreeMap<StepId, CollectionPolicyKind>,
    pub step_quorum_thresholds: std::collections::BTreeMap<StepId, u32>,
    pub step_target_counts: std::collections::BTreeMap<StepId, u32>,
    pub step_target_success_counts: std::collections::BTreeMap<StepId, u32>,
    pub step_target_terminal_failure_counts: std::collections::BTreeMap<StepId, u32>,
    pub target_retry_counts: std::collections::BTreeMap<String, u32>,
    pub failure_count: u32,
    pub consecutive_failure_count: u32,
    pub escalation_threshold: u32,
    pub max_step_retries: u32,
    pub ready_frames: Vec<FrameId>,
    pub ready_frame_membership: std::collections::BTreeSet<FrameId>,
    pub pending_body_frame_loops: Vec<LoopInstanceId>,
    pub pending_body_frame_loop_membership: std::collections::BTreeSet<LoopInstanceId>,
    pub active_node_count: u32,
    pub active_frame_count: u32,
    pub max_active_nodes: u32,
    pub max_active_frames: u32,
    pub max_frame_depth: u32,
    pub last_granted_frame: FrameId,
    pub last_granted_loop: LoopInstanceId,
}
impl Default for State {
    fn default() -> Self {
        initial_state()
    }
}

pub mod inputs {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CreateRun {
        pub step_ids: Vec<StepId>,
        pub ordered_steps: Vec<StepId>,
        pub step_has_conditions: std::collections::BTreeMap<StepId, bool>,
        pub step_dependencies: std::collections::BTreeMap<StepId, Vec<StepId>>,
        pub step_dependency_modes: std::collections::BTreeMap<StepId, DependencyMode>,
        pub step_branches: std::collections::BTreeMap<StepId, Option<BranchId>>,
        pub step_collection_policies: std::collections::BTreeMap<StepId, CollectionPolicyKind>,
        pub step_quorum_thresholds: std::collections::BTreeMap<StepId, u32>,
        pub escalation_threshold: u32,
        pub max_step_retries: u32,
        pub max_active_nodes: u32,
        pub max_active_frames: u32,
        pub max_frame_depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartRun {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct DispatchStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CompleteStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordStepOutput {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ConditionPassed {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ConditionRejected {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FailStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct SkipStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectFrameStepStatus {
        pub step_id: StepId,
        pub step_status: StepRunStatus,
        pub append_failure_ledger: bool,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CancelStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterTargets {
        pub step_id: StepId,
        pub target_count: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetSuccess {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetTerminalFailure {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetCanceled {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetFailure {
        pub step_id: StepId,
        pub target_id: MeerkatId,
        pub retry_key: String,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterReadyFrame {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PumpNodeScheduler {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterPendingBodyFrame {
        pub loop_instance_id: LoopInstanceId,
        pub depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PumpFrameScheduler {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct NodeExecutionReleased {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FrameTerminated {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TerminalizeCompleted {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TerminalizeFailed {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TerminalizeCanceled {}
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    CreateRun(inputs::CreateRun),
    StartRun(inputs::StartRun),
    DispatchStep(inputs::DispatchStep),
    CompleteStep(inputs::CompleteStep),
    RecordStepOutput(inputs::RecordStepOutput),
    ConditionPassed(inputs::ConditionPassed),
    ConditionRejected(inputs::ConditionRejected),
    FailStep(inputs::FailStep),
    SkipStep(inputs::SkipStep),
    ProjectFrameStepStatus(inputs::ProjectFrameStepStatus),
    CancelStep(inputs::CancelStep),
    RegisterTargets(inputs::RegisterTargets),
    RecordTargetSuccess(inputs::RecordTargetSuccess),
    RecordTargetTerminalFailure(inputs::RecordTargetTerminalFailure),
    RecordTargetCanceled(inputs::RecordTargetCanceled),
    RecordTargetFailure(inputs::RecordTargetFailure),
    RegisterReadyFrame(inputs::RegisterReadyFrame),
    PumpNodeScheduler(inputs::PumpNodeScheduler),
    RegisterPendingBodyFrame(inputs::RegisterPendingBodyFrame),
    PumpFrameScheduler(inputs::PumpFrameScheduler),
    NodeExecutionReleased(inputs::NodeExecutionReleased),
    FrameTerminated(inputs::FrameTerminated),
    TerminalizeCompleted(inputs::TerminalizeCompleted),
    TerminalizeFailed(inputs::TerminalizeFailed),
    TerminalizeCanceled(inputs::TerminalizeCanceled),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputKind {
    CreateRun,
    StartRun,
    DispatchStep,
    CompleteStep,
    RecordStepOutput,
    ConditionPassed,
    ConditionRejected,
    FailStep,
    SkipStep,
    ProjectFrameStepStatus,
    CancelStep,
    RegisterTargets,
    RecordTargetSuccess,
    RecordTargetTerminalFailure,
    RecordTargetCanceled,
    RecordTargetFailure,
    RegisterReadyFrame,
    PumpNodeScheduler,
    RegisterPendingBodyFrame,
    PumpFrameScheduler,
    NodeExecutionReleased,
    FrameTerminated,
    TerminalizeCompleted,
    TerminalizeFailed,
    TerminalizeCanceled,
}

pub mod effects {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EmitFlowRunNotice {
        pub run_status: FlowRunStatus,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EmitStepNotice {
        pub step_id: StepId,
        pub step_status: StepRunStatus,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct AppendFailureLedger {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PersistStepOutput {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct AdmitStepWork {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FlowTerminalized {
        pub run_status: FlowRunStatus,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EscalateSupervisor {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectTargetSuccess {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectTargetFailure {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectTargetCanceled {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct GrantNodeSlot {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct GrantBodyFrameStart {
        pub loop_instance_id: LoopInstanceId,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    EmitFlowRunNotice(effects::EmitFlowRunNotice),
    EmitStepNotice(effects::EmitStepNotice),
    AppendFailureLedger(effects::AppendFailureLedger),
    PersistStepOutput(effects::PersistStepOutput),
    AdmitStepWork(effects::AdmitStepWork),
    FlowTerminalized(effects::FlowTerminalized),
    EscalateSupervisor(effects::EscalateSupervisor),
    ProjectTargetSuccess(effects::ProjectTargetSuccess),
    ProjectTargetFailure(effects::ProjectTargetFailure),
    ProjectTargetCanceled(effects::ProjectTargetCanceled),
    GrantNodeSlot(effects::GrantNodeSlot),
    GrantBodyFrameStart(effects::GrantBodyFrameStart),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    EmitFlowRunNotice,
    EmitStepNotice,
    AppendFailureLedger,
    PersistStepOutput,
    AdmitStepWork,
    FlowTerminalized,
    EscalateSupervisor,
    ProjectTargetSuccess,
    ProjectTargetFailure,
    ProjectTargetCanceled,
    GrantNodeSlot,
    GrantBodyFrameStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionId {
    CreateRun,
    StartRun,
    DispatchStep,
    CompleteStep,
    RecordStepOutput,
    ConditionPassed,
    ConditionRejected,
    FailStep,
    SkipStep,
    ProjectFrameStepStatus,
    CancelStep,
    RegisterTargets,
    RecordTargetSuccess,
    RecordTargetTerminalFailure,
    RecordTargetCanceled,
    RecordTargetFailure,
    RegisterReadyFrame,
    PumpNodeScheduler,
    RegisterPendingBodyFrame,
    PumpFrameScheduler,
    NodeExecutionReleased,
    FrameTerminated,
    TerminalizeCompleted,
    TerminalizeFailed,
    TerminalizeCanceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuardId {
    Phase,
    StepExists,
    StepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperId {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub transition_id: TransitionId,
    pub next_state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionError {
    Refusal(TransitionRefusal),
    Kernel(KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionRefusal {
    NoMatchingTransition {
        phase: Phase,
        trigger: TriggerDiscriminant,
    },
    GuardRejected {
        rejections: Vec<GuardRejection>,
    },
    AmbiguousTransition {
        transitions: Vec<TransitionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerDiscriminant {
    Input(InputKind),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardRejection {
    pub transition_id: TransitionId,
    pub guard_id: GuardId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KernelError {
    ContextViolation {
        transition_id: TransitionId,
        detail: String,
    },
    HelperEvaluation {
        helper_id: HelperId,
        detail: String,
    },
    CodegenInvariant {
        detail: String,
    },
}

pub mod helpers {
    use super::*;
    pub fn none<C: Context>(_: &State, context: &C) -> Result<(), KernelError> {
        let _ = context;
        Ok(())
    }
}

pub fn initial_state() -> State {
    State {
        phase: Phase::Absent,
        tracked_steps: Default::default(),
        ordered_steps: Vec::new(),
        step_status: Default::default(),
        output_recorded: Default::default(),
        step_condition_results: Default::default(),
        step_has_conditions: Default::default(),
        step_dependencies: Default::default(),
        step_dependency_modes: Default::default(),
        step_branches: Default::default(),
        step_collection_policies: Default::default(),
        step_quorum_thresholds: Default::default(),
        step_target_counts: Default::default(),
        step_target_success_counts: Default::default(),
        step_target_terminal_failure_counts: Default::default(),
        target_retry_counts: Default::default(),
        failure_count: 0,
        consecutive_failure_count: 0,
        escalation_threshold: 0,
        max_step_retries: 0,
        ready_frames: Vec::new(),
        ready_frame_membership: Default::default(),
        pending_body_frame_loops: Vec::new(),
        pending_body_frame_loop_membership: Default::default(),
        active_node_count: 0,
        active_frame_count: 0,
        max_active_nodes: 0,
        max_active_frames: 0,
        max_frame_depth: 0,
        last_granted_frame: FrameId::from(String::new()),
        last_granted_loop: LoopInstanceId::from(String::new()),
    }
}

fn refusal(phase: &Phase, trigger: InputKind) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
        phase: phase.clone(),
        trigger: TriggerDiscriminant::Input(trigger),
    })
}

fn guard(transition_id: TransitionId, guard_id: GuardId) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::GuardRejected {
        rejections: vec![GuardRejection {
            transition_id,
            guard_id,
        }],
    })
}

fn ensure_step(
    state: &State,
    step_id: &StepId,
    transition_id: TransitionId,
) -> Result<Option<StepRunStatus>, TransitionError> {
    if !state.tracked_steps.contains(step_id) {
        return Err(guard(transition_id, GuardId::StepExists));
    }
    Ok(state.step_status.get(step_id).copied().flatten())
}

fn emit_run_notice(status: FlowRunStatus) -> Effect {
    Effect::EmitFlowRunNotice(effects::EmitFlowRunNotice { run_status: status })
}

fn emit_terminalized(status: FlowRunStatus) -> Effect {
    Effect::FlowTerminalized(effects::FlowTerminalized { run_status: status })
}

fn emit_step_notice(step_id: StepId, step_status: StepRunStatus) -> Effect {
    Effect::EmitStepNotice(effects::EmitStepNotice {
        step_id,
        step_status,
    })
}

fn update_step_status(
    state: &State,
    step_id: StepId,
    next_status: StepRunStatus,
    transition_id: TransitionId,
    allow_from: &[StepRunStatus],
) -> Result<State, TransitionError> {
    let current = ensure_step(state, &step_id, transition_id)?;
    if let Some(current_status) = current
        && !allow_from.contains(&current_status)
    {
        return Err(guard(transition_id, GuardId::StepStatus));
    }
    let mut next_state = state.clone();
    next_state.step_status.insert(step_id, Some(next_status));
    Ok(next_state)
}

fn maybe_add_unique<T: Ord + Clone>(
    seq: &mut Vec<T>,
    set: &mut std::collections::BTreeSet<T>,
    value: T,
) {
    if set.insert(value.clone()) {
        seq.push(value);
    }
}

pub fn transition<C: Context>(
    state: &State,
    input: Input,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match input {
        Input::CreateRun(payload) => {
            if state.phase != Phase::Absent {
                return Err(refusal(&state.phase, InputKind::CreateRun));
            }
            let mut next_state = initial_state();
            next_state.phase = Phase::Pending;
            next_state.tracked_steps = payload.step_ids.iter().cloned().collect();
            next_state.ordered_steps = payload.ordered_steps.clone();
            next_state.step_has_conditions = payload.step_has_conditions;
            next_state.step_dependencies = payload.step_dependencies;
            next_state.step_dependency_modes = payload.step_dependency_modes;
            next_state.step_branches = payload.step_branches;
            next_state.step_collection_policies = payload.step_collection_policies;
            next_state.step_quorum_thresholds = payload.step_quorum_thresholds;
            next_state.escalation_threshold = payload.escalation_threshold;
            next_state.max_step_retries = payload.max_step_retries;
            next_state.max_active_nodes = payload.max_active_nodes;
            next_state.max_active_frames = payload.max_active_frames;
            next_state.max_frame_depth = payload.max_frame_depth;
            for step_id in next_state.tracked_steps.clone() {
                next_state.step_status.insert(step_id.clone(), None);
                next_state.output_recorded.insert(step_id.clone(), false);
                next_state
                    .step_condition_results
                    .insert(step_id.clone(), None);
                next_state
                    .step_dependencies
                    .entry(step_id.clone())
                    .or_default();
                next_state
                    .step_dependency_modes
                    .entry(step_id.clone())
                    .or_insert(DependencyMode::All);
                next_state
                    .step_branches
                    .entry(step_id.clone())
                    .or_insert(None);
                next_state
                    .step_collection_policies
                    .entry(step_id.clone())
                    .or_insert(CollectionPolicyKind::All);
                next_state
                    .step_quorum_thresholds
                    .entry(step_id.clone())
                    .or_insert(0);
                next_state
                    .step_target_counts
                    .entry(step_id.clone())
                    .or_insert(0);
                next_state
                    .step_target_success_counts
                    .entry(step_id.clone())
                    .or_insert(0);
                next_state
                    .step_target_terminal_failure_counts
                    .entry(step_id)
                    .or_insert(0);
            }
            Ok(Outcome {
                transition_id: TransitionId::CreateRun,
                next_state,
                effects: vec![emit_run_notice(FlowRunStatus::Pending)],
            })
        }
        Input::StartRun(_) => {
            if state.phase != Phase::Pending {
                return Err(refusal(&state.phase, InputKind::StartRun));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Running;
            Ok(Outcome {
                transition_id: TransitionId::StartRun,
                next_state,
                effects: vec![emit_run_notice(FlowRunStatus::Running)],
            })
        }
        Input::DispatchStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::DispatchStep));
            }
            let next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Dispatched,
                TransitionId::DispatchStep,
                &[
                    StepRunStatus::Failed,
                    StepRunStatus::Skipped,
                    StepRunStatus::Completed,
                    StepRunStatus::Canceled,
                ][0..0],
            )?;
            Ok(Outcome {
                transition_id: TransitionId::DispatchStep,
                next_state,
                effects: vec![
                    emit_step_notice(payload.step_id.clone(), StepRunStatus::Dispatched),
                    Effect::AdmitStepWork(effects::AdmitStepWork {
                        step_id: payload.step_id,
                    }),
                ],
            })
        }
        Input::CompleteStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::CompleteStep));
            }
            let mut next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Completed,
                TransitionId::CompleteStep,
                &[StepRunStatus::Dispatched],
            )?;
            next_state.consecutive_failure_count = 0;
            Ok(Outcome {
                transition_id: TransitionId::CompleteStep,
                next_state,
                effects: vec![emit_step_notice(payload.step_id, StepRunStatus::Completed)],
            })
        }
        Input::RecordStepOutput(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordStepOutput));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordStepOutput)?;
            let mut next_state = state.clone();
            next_state
                .output_recorded
                .insert(payload.step_id.clone(), true);
            Ok(Outcome {
                transition_id: TransitionId::RecordStepOutput,
                next_state,
                effects: vec![Effect::PersistStepOutput(effects::PersistStepOutput {
                    step_id: payload.step_id,
                })],
            })
        }
        Input::ConditionPassed(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::ConditionPassed));
            }
            ensure_step(state, &payload.step_id, TransitionId::ConditionPassed)?;
            let mut next_state = state.clone();
            next_state
                .step_condition_results
                .insert(payload.step_id, Some(true));
            Ok(Outcome {
                transition_id: TransitionId::ConditionPassed,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::ConditionRejected(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::ConditionRejected));
            }
            ensure_step(state, &payload.step_id, TransitionId::ConditionRejected)?;
            let mut next_state = state.clone();
            next_state
                .step_condition_results
                .insert(payload.step_id, Some(false));
            Ok(Outcome {
                transition_id: TransitionId::ConditionRejected,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::FailStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::FailStep));
            }
            let mut next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Failed,
                TransitionId::FailStep,
                &[StepRunStatus::Dispatched],
            )?;
            next_state.failure_count = next_state.failure_count.saturating_add(1);
            next_state.consecutive_failure_count =
                next_state.consecutive_failure_count.saturating_add(1);
            let should_escalate = next_state.escalation_threshold > 0
                && next_state.consecutive_failure_count >= next_state.escalation_threshold;
            let mut effects = vec![
                emit_step_notice(payload.step_id.clone(), StepRunStatus::Failed),
                Effect::AppendFailureLedger(effects::AppendFailureLedger {
                    step_id: payload.step_id.clone(),
                }),
            ];
            if should_escalate {
                effects.push(Effect::EscalateSupervisor(effects::EscalateSupervisor {
                    step_id: payload.step_id.clone(),
                }));
            }
            Ok(Outcome {
                transition_id: TransitionId::FailStep,
                next_state,
                effects,
            })
        }
        Input::SkipStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::SkipStep));
            }
            let next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Skipped,
                TransitionId::SkipStep,
                &[StepRunStatus::Dispatched],
            )?;
            Ok(Outcome {
                transition_id: TransitionId::SkipStep,
                next_state,
                effects: vec![emit_step_notice(payload.step_id, StepRunStatus::Skipped)],
            })
        }
        Input::ProjectFrameStepStatus(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::ProjectFrameStepStatus));
            }
            ensure_step(
                state,
                &payload.step_id,
                TransitionId::ProjectFrameStepStatus,
            )?;
            let mut next_state = state.clone();
            next_state
                .step_status
                .insert(payload.step_id.clone(), Some(payload.step_status));
            let mut effects = vec![emit_step_notice(
                payload.step_id.clone(),
                payload.step_status,
            )];
            if payload.step_status == StepRunStatus::Failed {
                next_state.failure_count = next_state.failure_count.saturating_add(1);
                next_state.consecutive_failure_count =
                    next_state.consecutive_failure_count.saturating_add(1);
                if payload.append_failure_ledger {
                    effects.push(Effect::AppendFailureLedger(effects::AppendFailureLedger {
                        step_id: payload.step_id.clone(),
                    }));
                }
                if next_state.escalation_threshold > 0
                    && next_state.consecutive_failure_count >= next_state.escalation_threshold
                {
                    effects.push(Effect::EscalateSupervisor(effects::EscalateSupervisor {
                        step_id: payload.step_id.clone(),
                    }));
                }
            } else if payload.step_status == StepRunStatus::Completed {
                next_state.consecutive_failure_count = 0;
            }
            Ok(Outcome {
                transition_id: TransitionId::ProjectFrameStepStatus,
                next_state,
                effects,
            })
        }
        Input::CancelStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::CancelStep));
            }
            let next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Canceled,
                TransitionId::CancelStep,
                &[StepRunStatus::Dispatched],
            )?;
            Ok(Outcome {
                transition_id: TransitionId::CancelStep,
                next_state,
                effects: vec![emit_step_notice(payload.step_id, StepRunStatus::Canceled)],
            })
        }
        Input::RegisterTargets(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RegisterTargets));
            }
            ensure_step(state, &payload.step_id, TransitionId::RegisterTargets)?;
            let mut next_state = state.clone();
            next_state
                .step_target_counts
                .insert(payload.step_id, payload.target_count);
            Ok(Outcome {
                transition_id: TransitionId::RegisterTargets,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::RecordTargetSuccess(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordTargetSuccess));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetSuccess)?;
            let mut next_state = state.clone();
            let entry = next_state
                .step_target_success_counts
                .entry(payload.step_id.clone())
                .or_insert(0);
            *entry = entry.saturating_add(1);
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetSuccess,
                next_state,
                effects: vec![Effect::ProjectTargetSuccess(
                    effects::ProjectTargetSuccess {
                        step_id: payload.step_id,
                        target_id: payload.target_id,
                    },
                )],
            })
        }
        Input::RecordTargetTerminalFailure(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(
                    &state.phase,
                    InputKind::RecordTargetTerminalFailure,
                ));
            }
            ensure_step(
                state,
                &payload.step_id,
                TransitionId::RecordTargetTerminalFailure,
            )?;
            let mut next_state = state.clone();
            let entry = next_state
                .step_target_terminal_failure_counts
                .entry(payload.step_id)
                .or_insert(0);
            *entry = entry.saturating_add(1);
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetTerminalFailure,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::RecordTargetCanceled(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordTargetCanceled));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetCanceled)?;
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetCanceled,
                next_state: state.clone(),
                effects: vec![Effect::ProjectTargetCanceled(
                    effects::ProjectTargetCanceled {
                        step_id: payload.step_id,
                        target_id: payload.target_id,
                    },
                )],
            })
        }
        Input::RecordTargetFailure(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordTargetFailure));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetFailure)?;
            let mut next_state = state.clone();
            let entry = next_state
                .target_retry_counts
                .entry(payload.retry_key)
                .or_insert(0);
            *entry = entry.saturating_add(1);
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetFailure,
                next_state,
                effects: vec![Effect::ProjectTargetFailure(
                    effects::ProjectTargetFailure {
                        step_id: payload.step_id,
                        target_id: payload.target_id,
                    },
                )],
            })
        }
        Input::RegisterReadyFrame(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RegisterReadyFrame));
            }
            let mut next_state = state.clone();
            maybe_add_unique(
                &mut next_state.ready_frames,
                &mut next_state.ready_frame_membership,
                payload.frame_id,
            );
            Ok(Outcome {
                transition_id: TransitionId::RegisterReadyFrame,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::PumpNodeScheduler(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::PumpNodeScheduler));
            }
            let mut next_state = state.clone();
            if next_state.ready_frames.is_empty() {
                return Err(guard(TransitionId::PumpNodeScheduler, GuardId::StepStatus));
            }
            if next_state.max_active_nodes > 0
                && next_state.active_node_count >= next_state.max_active_nodes
            {
                return Err(guard(TransitionId::PumpNodeScheduler, GuardId::StepStatus));
            }
            let frame_id = next_state.ready_frames.remove(0);
            next_state.ready_frame_membership.remove(&frame_id);
            next_state.active_node_count = next_state.active_node_count.saturating_add(1);
            next_state.last_granted_frame = frame_id.clone();
            Ok(Outcome {
                transition_id: TransitionId::PumpNodeScheduler,
                next_state,
                effects: vec![Effect::GrantNodeSlot(effects::GrantNodeSlot { frame_id })],
            })
        }
        Input::RegisterPendingBodyFrame(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RegisterPendingBodyFrame));
            }
            let mut next_state = state.clone();
            maybe_add_unique(
                &mut next_state.pending_body_frame_loops,
                &mut next_state.pending_body_frame_loop_membership,
                payload.loop_instance_id,
            );
            Ok(Outcome {
                transition_id: TransitionId::RegisterPendingBodyFrame,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::PumpFrameScheduler(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::PumpFrameScheduler));
            }
            let mut next_state = state.clone();
            if next_state.pending_body_frame_loops.is_empty() {
                return Err(guard(TransitionId::PumpFrameScheduler, GuardId::StepStatus));
            }
            if next_state.max_active_frames > 0
                && next_state.active_frame_count >= next_state.max_active_frames
            {
                return Err(guard(TransitionId::PumpFrameScheduler, GuardId::StepStatus));
            }
            let loop_instance_id = next_state.pending_body_frame_loops.remove(0);
            next_state
                .pending_body_frame_loop_membership
                .remove(&loop_instance_id);
            next_state.active_frame_count = next_state.active_frame_count.saturating_add(1);
            next_state.last_granted_loop = loop_instance_id.clone();
            Ok(Outcome {
                transition_id: TransitionId::PumpFrameScheduler,
                next_state,
                effects: vec![Effect::GrantBodyFrameStart(effects::GrantBodyFrameStart {
                    loop_instance_id,
                })],
            })
        }
        Input::NodeExecutionReleased(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::NodeExecutionReleased));
            }
            let mut next_state = state.clone();
            next_state.active_node_count = next_state.active_node_count.saturating_sub(1);
            Ok(Outcome {
                transition_id: TransitionId::NodeExecutionReleased,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::FrameTerminated(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::FrameTerminated));
            }
            let mut next_state = state.clone();
            next_state.active_frame_count = next_state.active_frame_count.saturating_sub(1);
            Ok(Outcome {
                transition_id: TransitionId::FrameTerminated,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::TerminalizeCompleted(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::TerminalizeCompleted));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Completed;
            Ok(Outcome {
                transition_id: TransitionId::TerminalizeCompleted,
                next_state,
                effects: vec![
                    emit_run_notice(FlowRunStatus::Completed),
                    emit_terminalized(FlowRunStatus::Completed),
                ],
            })
        }
        Input::TerminalizeFailed(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::TerminalizeFailed));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Failed;
            Ok(Outcome {
                transition_id: TransitionId::TerminalizeFailed,
                next_state,
                effects: vec![
                    emit_run_notice(FlowRunStatus::Failed),
                    emit_terminalized(FlowRunStatus::Failed),
                ],
            })
        }
        Input::TerminalizeCanceled(_) => {
            if matches!(state.phase, Phase::Completed | Phase::Canceled) {
                return Err(refusal(&state.phase, InputKind::TerminalizeCanceled));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Canceled;
            Ok(Outcome {
                transition_id: TransitionId::TerminalizeCanceled,
                next_state,
                effects: vec![
                    emit_run_notice(FlowRunStatus::Canceled),
                    emit_terminalized(FlowRunStatus::Canceled),
                ],
            })
        }
    }
}
