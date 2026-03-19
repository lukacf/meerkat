//! RMAT/MTAS sealed authority for the RuntimeControl machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all RuntimeControl state mutations flow through the machine authority.
//! Handwritten shell code calls [`RuntimeControlAuthority::apply`] and executes
//! returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/runtime_control.rs`:
//!
//! - 8 states: Initializing, Idle, Attached, Running, Recovering, Retired, Stopped, Destroyed
//! - 20 input variants (Initialize, AttachExecutor, DetachExecutor, BeginRun,
//!   RunCompleted/Failed/Cancelled, SubmitWork, AdmissionAccepted/Rejected/Deduplicated,
//!   RecoverRequested, RecoverySucceeded, RetireRequested, ResetRequested,
//!   StopRequested, DestroyRequested, ResumeRequested, ExternalToolDeltaReceived,
//!   RecycleRequested, RecycleSucceeded)
//! - 4 fields: current_run_id, pre_run_state, wake_pending, process_pending
//! - 2 invariants: running_implies_active_run, active_run_only_while_running_or_retired
//! - 8 effects: ResolveAdmission, SubmitAdmittedIngressEffect, SubmitRunPrimitive,
//!   SignalWake, SignalImmediateProcess, EmitRuntimeNotice,
//!   ResolveCompletionAsTerminated, ApplyControlPlaneCommand, InitiateRecycle

use meerkat_core::lifecycle::RunId;

use crate::runtime_state::{RuntimeState, RuntimeStateTransitionError};

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the RuntimeControl machine.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`RuntimeControlAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeControlInput {
    Initialize,
    AttachExecutor,
    DetachExecutor,
    BeginRun {
        run_id: RunId,
    },
    RunCompleted {
        run_id: RunId,
    },
    RunFailed {
        run_id: RunId,
    },
    RunCancelled {
        run_id: RunId,
    },
    SubmitWork {
        work_id: String,
    },
    AdmissionAccepted {
        work_id: String,
        handling_mode: HandlingMode,
    },
    AdmissionRejected {
        work_id: String,
        reason: String,
    },
    AdmissionDeduplicated {
        work_id: String,
        existing_work_id: String,
    },
    RecoverRequested,
    RecoverySucceeded,
    RetireRequested,
    ResetRequested,
    StopRequested,
    DestroyRequested,
    ResumeRequested,
    ExternalToolDeltaReceived,
    RecycleRequested,
    RecycleSucceeded,
}

/// Handling mode for admitted work — determines wake/process signaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlingMode {
    Queue,
    Steer,
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by RuntimeControl transitions.
///
/// Shell code receives these from [`RuntimeControlAuthority::apply`] and is
/// responsible for executing the side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeControlEffect {
    ResolveAdmission {
        work_id: String,
    },
    SubmitAdmittedIngressEffect {
        work_id: String,
        handling_mode: HandlingMode,
    },
    SubmitRunPrimitive {
        run_id: RunId,
    },
    SignalWake,
    SignalImmediateProcess,
    EmitRuntimeNotice {
        kind: String,
        detail: String,
    },
    ResolveCompletionAsTerminated {
        reason: String,
    },
    ApplyControlPlaneCommand {
        command: String,
    },
    InitiateRecycle,
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the RuntimeControl authority.
#[derive(Debug)]
pub struct RuntimeControlTransition {
    /// The phase before the transition.
    pub from_phase: RuntimeState,
    /// The phase after the transition.
    pub next_phase: RuntimeState,
    /// Effects to be executed by shell code.
    pub effects: Vec<RuntimeControlEffect>,
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

/// Canonical machine-owned fields for RuntimeControl.
#[derive(Debug, Clone)]
struct RuntimeControlFields {
    current_run_id: Option<RunId>,
    pre_run_state: Option<RuntimeState>,
    wake_pending: bool,
    process_pending: bool,
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for RuntimeControl state mutation.
///
/// Only [`RuntimeControlAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for runtime control state.
pub trait RuntimeControlMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(
        &mut self,
        input: RuntimeControlInput,
    ) -> Result<RuntimeControlTransition, RuntimeStateTransitionError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for RuntimeControl state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table.
#[derive(Debug, Clone)]
pub struct RuntimeControlAuthority {
    /// Canonical phase.
    phase: RuntimeState,
    /// Canonical machine-owned fields.
    fields: RuntimeControlFields,
}

impl sealed::Sealed for RuntimeControlAuthority {}

impl RuntimeControlAuthority {
    /// Create an authority in the Initializing state.
    pub fn new() -> Self {
        Self {
            phase: RuntimeState::Initializing,
            fields: RuntimeControlFields {
                current_run_id: None,
                pre_run_state: None,
                wake_pending: false,
                process_pending: false,
            },
        }
    }

    /// Create an authority initialized to a specific phase (for recovery).
    pub fn from_state(phase: RuntimeState) -> Self {
        Self {
            phase,
            fields: RuntimeControlFields {
                current_run_id: None,
                pre_run_state: None,
                wake_pending: false,
                process_pending: false,
            },
        }
    }

    /// Current phase (read from canonical state).
    pub fn phase(&self) -> RuntimeState {
        self.phase
    }

    /// Current run ID from canonical state.
    pub fn current_run_id(&self) -> Option<&RunId> {
        self.fields.current_run_id.as_ref()
    }

    /// Pre-run state from canonical state.
    pub fn pre_run_state(&self) -> Option<RuntimeState> {
        self.fields.pre_run_state
    }

    /// Whether a wake is pending.
    pub fn wake_pending(&self) -> bool {
        self.fields.wake_pending
    }

    /// Whether immediate processing is pending.
    pub fn process_pending(&self) -> bool {
        self.fields.process_pending
    }

    /// Check if the runtime is in the Idle state.
    pub fn is_idle(&self) -> bool {
        self.phase == RuntimeState::Idle
    }

    /// Check if the runtime is in the Attached state.
    pub fn is_attached(&self) -> bool {
        self.phase == RuntimeState::Attached
    }

    /// Check if the runtime is in the Running state.
    pub fn is_running(&self) -> bool {
        self.phase == RuntimeState::Running
    }

    /// Check if the runtime can process queued inputs.
    pub fn can_process_queue(&self) -> bool {
        self.phase.can_process_queue()
    }

    /// Check if a transition is legal without applying it.
    pub fn can_accept(&self, input: &RuntimeControlInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &RuntimeControlInput,
    ) -> Result<
        (
            RuntimeState,
            RuntimeControlFields,
            Vec<RuntimeControlEffect>,
        ),
        RuntimeStateTransitionError,
    > {
        use RuntimeControlInput::{
            AdmissionAccepted, AdmissionDeduplicated, AdmissionRejected, AttachExecutor, BeginRun,
            DestroyRequested, DetachExecutor, ExternalToolDeltaReceived, Initialize,
            RecoverRequested, RecoverySucceeded, RecycleRequested, RecycleSucceeded,
            ResetRequested, ResumeRequested, RetireRequested, RunCancelled, RunCompleted,
            RunFailed, StopRequested, SubmitWork,
        };
        use RuntimeState::{
            Attached, Destroyed, Idle, Initializing, Recovering, Retired, Running, Stopped,
        };

        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
            // Initialize: Initializing -> Idle
            (Initializing, Initialize) => Idle,

            // AttachExecutor: Idle -> Attached
            (Idle, AttachExecutor) => Attached,

            // DetachExecutor: Attached -> Idle
            (Attached, DetachExecutor) => Idle,

            // BeginRun from Idle
            (Idle, BeginRun { run_id }) => {
                if fields.current_run_id.is_some() {
                    return Err(RuntimeStateTransitionError {
                        from: phase,
                        to: Running,
                    });
                }
                fields.current_run_id = Some(run_id.clone());
                fields.pre_run_state = Some(Idle);
                fields.wake_pending = false;
                fields.process_pending = false;
                effects.push(RuntimeControlEffect::SubmitRunPrimitive {
                    run_id: run_id.clone(),
                });
                Running
            }

            // BeginRun from Attached
            (Attached, BeginRun { run_id }) => {
                if fields.current_run_id.is_some() {
                    return Err(RuntimeStateTransitionError {
                        from: phase,
                        to: Running,
                    });
                }
                fields.current_run_id = Some(run_id.clone());
                fields.pre_run_state = Some(Attached);
                fields.wake_pending = false;
                fields.process_pending = false;
                effects.push(RuntimeControlEffect::SubmitRunPrimitive {
                    run_id: run_id.clone(),
                });
                Running
            }

            // BeginRun from Retired (drain cycle)
            (Retired, BeginRun { run_id }) => {
                if fields.current_run_id.is_some() {
                    return Err(RuntimeStateTransitionError {
                        from: phase,
                        to: Running,
                    });
                }
                fields.current_run_id = Some(run_id.clone());
                fields.pre_run_state = Some(Retired);
                fields.wake_pending = false;
                fields.process_pending = false;
                effects.push(RuntimeControlEffect::SubmitRunPrimitive {
                    run_id: run_id.clone(),
                });
                Running
            }

            // BeginRun from Recovering (recovery-then-run path)
            (Recovering, BeginRun { run_id }) => {
                if fields.current_run_id.is_some() {
                    return Err(RuntimeStateTransitionError {
                        from: phase,
                        to: Running,
                    });
                }
                fields.current_run_id = Some(run_id.clone());
                fields.pre_run_state = Some(Recovering);
                fields.wake_pending = false;
                fields.process_pending = false;
                effects.push(RuntimeControlEffect::SubmitRunPrimitive {
                    run_id: run_id.clone(),
                });
                Running
            }

            // RunCompleted from Running — split by pre_run_state
            (Running, RunCompleted { run_id }) => {
                self.validate_run_terminal(run_id, &fields)?;
                let target = self.resolve_run_return(&fields);
                fields.current_run_id = None;
                fields.pre_run_state = None;
                target
            }

            // RunFailed from Running — split by pre_run_state
            (Running, RunFailed { run_id }) => {
                self.validate_run_terminal(run_id, &fields)?;
                let target = self.resolve_run_return(&fields);
                fields.current_run_id = None;
                fields.pre_run_state = None;
                target
            }

            // RunCancelled from Running — split by pre_run_state
            (Running, RunCancelled { run_id }) => {
                self.validate_run_terminal(run_id, &fields)?;
                let target = self.resolve_run_return(&fields);
                fields.current_run_id = None;
                fields.pre_run_state = None;
                target
            }

            // SubmitWork from Idle/Running/Attached — stays in same phase, emits ResolveAdmission
            (Idle, SubmitWork { work_id }) => {
                effects.push(RuntimeControlEffect::ResolveAdmission {
                    work_id: work_id.clone(),
                });
                Idle
            }
            (Running, SubmitWork { work_id }) => {
                effects.push(RuntimeControlEffect::ResolveAdmission {
                    work_id: work_id.clone(),
                });
                Running
            }
            (Attached, SubmitWork { work_id }) => {
                effects.push(RuntimeControlEffect::ResolveAdmission {
                    work_id: work_id.clone(),
                });
                Attached
            }

            // AdmissionAccepted — stays in same phase, emits SubmitAdmittedIngressEffect + signals
            (
                Idle,
                AdmissionAccepted {
                    work_id,
                    handling_mode,
                },
            ) => {
                let process_immediately = *handling_mode == HandlingMode::Steer;
                // Idle/Attached => wake_when_idle = true
                fields.wake_pending = true;
                fields.process_pending = process_immediately;
                effects.push(RuntimeControlEffect::SubmitAdmittedIngressEffect {
                    work_id: work_id.clone(),
                    handling_mode: *handling_mode,
                });
                effects.push(RuntimeControlEffect::SignalWake);
                if process_immediately {
                    effects.push(RuntimeControlEffect::SignalImmediateProcess);
                }
                Idle
            }
            (
                Attached,
                AdmissionAccepted {
                    work_id,
                    handling_mode,
                },
            ) => {
                let process_immediately = *handling_mode == HandlingMode::Steer;
                // Attached => wake_when_idle = true
                fields.wake_pending = true;
                fields.process_pending = process_immediately;
                effects.push(RuntimeControlEffect::SubmitAdmittedIngressEffect {
                    work_id: work_id.clone(),
                    handling_mode: *handling_mode,
                });
                effects.push(RuntimeControlEffect::SignalWake);
                if process_immediately {
                    effects.push(RuntimeControlEffect::SignalImmediateProcess);
                }
                Attached
            }
            (
                Running,
                AdmissionAccepted {
                    work_id,
                    handling_mode,
                },
            ) => {
                let process_immediately = *handling_mode == HandlingMode::Steer;
                // Running => wake_when_idle = false
                fields.wake_pending = process_immediately;
                fields.process_pending = process_immediately;
                effects.push(RuntimeControlEffect::SubmitAdmittedIngressEffect {
                    work_id: work_id.clone(),
                    handling_mode: *handling_mode,
                });
                if process_immediately {
                    effects.push(RuntimeControlEffect::SignalWake);
                    effects.push(RuntimeControlEffect::SignalImmediateProcess);
                }
                Running
            }

            // AdmissionRejected — stays in same phase, emits EmitRuntimeNotice
            (Idle, AdmissionRejected { reason, .. }) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "AdmissionRejected".into(),
                    detail: reason.clone(),
                });
                Idle
            }
            (Running, AdmissionRejected { reason, .. }) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "AdmissionRejected".into(),
                    detail: reason.clone(),
                });
                Running
            }
            (Attached, AdmissionRejected { reason, .. }) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "AdmissionRejected".into(),
                    detail: reason.clone(),
                });
                Attached
            }

            // AdmissionDeduplicated — stays in same phase, emits EmitRuntimeNotice
            (Idle, AdmissionDeduplicated { .. }) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "AdmissionDeduplicated".into(),
                    detail: "ExistingInputLinked".into(),
                });
                Idle
            }
            (Running, AdmissionDeduplicated { .. }) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "AdmissionDeduplicated".into(),
                    detail: "ExistingInputLinked".into(),
                });
                Running
            }
            (Attached, AdmissionDeduplicated { .. }) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "AdmissionDeduplicated".into(),
                    detail: "ExistingInputLinked".into(),
                });
                Attached
            }

            // RecoverRequested from Idle
            (Idle, RecoverRequested) => {
                fields.pre_run_state = Some(Idle);
                Recovering
            }
            // RecoverRequested from Running
            (Running, RecoverRequested) => {
                fields.current_run_id = None;
                fields.pre_run_state = Some(Running);
                Recovering
            }
            // RecoverRequested from Attached
            (Attached, RecoverRequested) => {
                fields.pre_run_state = Some(Attached);
                Recovering
            }

            // RecoverySucceeded from Recovering
            (Recovering, RecoverySucceeded) => {
                fields.current_run_id = None;
                fields.pre_run_state = None;
                fields.wake_pending = false;
                fields.process_pending = false;
                Idle
            }

            // RetireRequested from Idle/Running/Attached
            (Idle, RetireRequested) => {
                effects.push(RuntimeControlEffect::ApplyControlPlaneCommand {
                    command: "Retire".into(),
                });
                Retired
            }
            (Running, RetireRequested) => {
                effects.push(RuntimeControlEffect::ApplyControlPlaneCommand {
                    command: "Retire".into(),
                });
                Retired
            }
            (Attached, RetireRequested) => {
                effects.push(RuntimeControlEffect::ApplyControlPlaneCommand {
                    command: "Retire".into(),
                });
                Retired
            }

            // ResetRequested from Initializing/Idle/Attached/Recovering/Retired
            (Initializing | Idle | Attached | Recovering | Retired, ResetRequested) => {
                fields.current_run_id = None;
                fields.pre_run_state = None;
                fields.wake_pending = false;
                fields.process_pending = false;
                effects.push(RuntimeControlEffect::ApplyControlPlaneCommand {
                    command: "Reset".into(),
                });
                effects.push(RuntimeControlEffect::ResolveCompletionAsTerminated {
                    reason: "Reset".into(),
                });
                Idle
            }

            // StopRequested from all non-terminal states
            (Initializing | Idle | Attached | Running | Recovering | Retired, StopRequested) => {
                fields.current_run_id = None;
                fields.pre_run_state = None;
                effects.push(RuntimeControlEffect::ApplyControlPlaneCommand {
                    command: "Stop".into(),
                });
                effects.push(RuntimeControlEffect::ResolveCompletionAsTerminated {
                    reason: "Stopped".into(),
                });
                Stopped
            }

            // DestroyRequested from all non-terminal states + Stopped
            (
                Initializing | Idle | Attached | Running | Recovering | Retired | Stopped,
                DestroyRequested,
            ) => {
                fields.current_run_id = None;
                fields.pre_run_state = None;
                effects.push(RuntimeControlEffect::ApplyControlPlaneCommand {
                    command: "Destroy".into(),
                });
                effects.push(RuntimeControlEffect::ResolveCompletionAsTerminated {
                    reason: "Destroyed".into(),
                });
                Destroyed
            }

            // ResumeRequested from Recovering
            (Recovering, ResumeRequested) => Idle,

            // ExternalToolDeltaReceived — stays in same phase
            (Idle, ExternalToolDeltaReceived) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "ExternalToolDelta".into(),
                    detail: "Received".into(),
                });
                Idle
            }
            (Running, ExternalToolDeltaReceived) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "ExternalToolDelta".into(),
                    detail: "Received".into(),
                });
                Running
            }
            (Recovering, ExternalToolDeltaReceived) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "ExternalToolDelta".into(),
                    detail: "Received".into(),
                });
                Recovering
            }
            (Retired, ExternalToolDeltaReceived) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "ExternalToolDelta".into(),
                    detail: "Received".into(),
                });
                Retired
            }
            (Attached, ExternalToolDeltaReceived) => {
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "ExternalToolDelta".into(),
                    detail: "Received".into(),
                });
                Attached
            }

            // RecycleRequested from Retired/Idle/Attached (guard: no_active_run)
            (Retired | Idle | Attached, RecycleRequested) => {
                if fields.current_run_id.is_some() {
                    return Err(RuntimeStateTransitionError {
                        from: phase,
                        to: Recovering,
                    });
                }
                fields.pre_run_state = Some(phase);
                effects.push(RuntimeControlEffect::InitiateRecycle);
                Recovering
            }

            // RecycleSucceeded from Recovering
            (Recovering, RecycleSucceeded) => {
                fields.current_run_id = None;
                fields.pre_run_state = None;
                fields.wake_pending = false;
                fields.process_pending = false;
                effects.push(RuntimeControlEffect::EmitRuntimeNotice {
                    kind: "Recycle".into(),
                    detail: "Succeeded".into(),
                });
                Idle
            }

            // All other combinations are illegal.
            _ => {
                return Err(RuntimeStateTransitionError {
                    from: phase,
                    to: self.infer_target(input),
                });
            }
        };

        Ok((next_phase, fields, effects))
    }

    /// Validate that a run terminal input (Completed/Failed/Cancelled) matches the active run.
    fn validate_run_terminal(
        &self,
        run_id: &RunId,
        fields: &RuntimeControlFields,
    ) -> Result<(), RuntimeStateTransitionError> {
        match &fields.current_run_id {
            Some(active_id) if active_id == run_id => Ok(()),
            _ => Err(RuntimeStateTransitionError {
                from: self.phase,
                to: self.resolve_run_return(fields),
            }),
        }
    }

    /// Resolve the return state after a run terminal event.
    /// Uses pre_run_state to determine where to return:
    /// - Some(Retired) -> Retired
    /// - Some(Attached) -> Attached
    /// - Some(Idle) | None -> Idle
    fn resolve_run_return(&self, fields: &RuntimeControlFields) -> RuntimeState {
        match fields.pre_run_state {
            Some(RuntimeState::Retired) => RuntimeState::Retired,
            Some(RuntimeState::Attached) => RuntimeState::Attached,
            _ => RuntimeState::Idle,
        }
    }

    /// Infer a target state for error reporting when a transition is illegal.
    fn infer_target(&self, input: &RuntimeControlInput) -> RuntimeState {
        use RuntimeControlInput::{
            AdmissionAccepted, AdmissionDeduplicated, AdmissionRejected, AttachExecutor, BeginRun,
            DestroyRequested, DetachExecutor, ExternalToolDeltaReceived, Initialize,
            RecoverRequested, RecoverySucceeded, RecycleRequested, RecycleSucceeded,
            ResetRequested, ResumeRequested, RetireRequested, RunCancelled, RunCompleted,
            RunFailed, StopRequested, SubmitWork,
        };
        match input {
            Initialize => RuntimeState::Idle,
            AttachExecutor => RuntimeState::Attached,
            DetachExecutor => RuntimeState::Idle,
            BeginRun { .. } => RuntimeState::Running,
            RunCompleted { .. } | RunFailed { .. } | RunCancelled { .. } => {
                self.resolve_run_return(&self.fields)
            }
            SubmitWork { .. }
            | AdmissionAccepted { .. }
            | AdmissionRejected { .. }
            | AdmissionDeduplicated { .. }
            | ExternalToolDeltaReceived => self.phase,
            RecoverRequested | RecycleRequested => RuntimeState::Recovering,
            RecoverySucceeded | RecycleSucceeded | ResumeRequested | ResetRequested => {
                RuntimeState::Idle
            }
            RetireRequested => RuntimeState::Retired,
            StopRequested => RuntimeState::Stopped,
            DestroyRequested => RuntimeState::Destroyed,
        }
    }
}

impl RuntimeControlMutator for RuntimeControlAuthority {
    fn apply(
        &mut self,
        input: RuntimeControlInput,
    ) -> Result<RuntimeControlTransition, RuntimeStateTransitionError> {
        let from_phase = self.phase;
        let (next_phase, next_fields, effects) = self.evaluate(&input)?;

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;

        Ok(RuntimeControlTransition {
            from_phase,
            next_phase,
            effects,
        })
    }
}

impl Default for RuntimeControlAuthority {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn make_idle() -> RuntimeControlAuthority {
        let mut auth = RuntimeControlAuthority::new();
        auth.apply(RuntimeControlInput::Initialize).unwrap();
        auth
    }

    fn make_attached() -> RuntimeControlAuthority {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::AttachExecutor).unwrap();
        auth
    }

    fn make_running_from_idle() -> (RuntimeControlAuthority, RunId) {
        let mut auth = make_idle();
        let run_id = RunId::new();
        auth.apply(RuntimeControlInput::BeginRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        (auth, run_id)
    }

    fn make_running_from_attached() -> (RuntimeControlAuthority, RunId) {
        let mut auth = make_attached();
        let run_id = RunId::new();
        auth.apply(RuntimeControlInput::BeginRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        (auth, run_id)
    }

    // --- Initialize ---

    #[test]
    fn initialize_transitions_to_idle() {
        let mut auth = RuntimeControlAuthority::new();
        let t = auth.apply(RuntimeControlInput::Initialize).unwrap();
        assert_eq!(t.from_phase, RuntimeState::Initializing);
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert_eq!(auth.phase(), RuntimeState::Idle);
    }

    #[test]
    fn initialize_rejected_from_idle() {
        let mut auth = make_idle();
        assert!(auth.apply(RuntimeControlInput::Initialize).is_err());
    }

    // --- Attach / Detach ---

    #[test]
    fn attach_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::AttachExecutor).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Attached);
        assert!(auth.is_attached());
    }

    #[test]
    fn attach_rejected_from_initializing() {
        let mut auth = RuntimeControlAuthority::new();
        assert!(auth.apply(RuntimeControlInput::AttachExecutor).is_err());
    }

    #[test]
    fn detach_from_attached() {
        let mut auth = make_attached();
        let t = auth.apply(RuntimeControlInput::DetachExecutor).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(auth.is_idle());
    }

    #[test]
    fn detach_rejected_from_idle() {
        let mut auth = make_idle();
        assert!(auth.apply(RuntimeControlInput::DetachExecutor).is_err());
    }

    // --- BeginRun ---

    #[test]
    fn begin_run_from_idle() {
        let run_id = RunId::new();
        let mut auth = make_idle();
        let t = auth
            .apply(RuntimeControlInput::BeginRun {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Running);
        assert_eq!(auth.current_run_id(), Some(&run_id));
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Idle));
        assert_eq!(t.effects.len(), 1);
        assert!(matches!(
            &t.effects[0],
            RuntimeControlEffect::SubmitRunPrimitive { run_id: r } if r == &run_id
        ));
    }

    #[test]
    fn begin_run_from_attached() {
        let run_id = RunId::new();
        let mut auth = make_attached();
        let t = auth
            .apply(RuntimeControlInput::BeginRun {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Running);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Attached));
    }

    #[test]
    fn begin_run_from_retired() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        let run_id = RunId::new();
        let t = auth
            .apply(RuntimeControlInput::BeginRun {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Running);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Retired));
    }

    #[test]
    fn begin_run_rejected_from_running() {
        let (mut auth, _) = make_running_from_idle();
        assert!(
            auth.apply(RuntimeControlInput::BeginRun {
                run_id: RunId::new()
            })
            .is_err()
        );
    }

    #[test]
    fn begin_run_rejected_with_active_run() {
        // This shouldn't be reachable normally, but validates the guard
        let mut auth = make_idle();
        let run_id = RunId::new();
        auth.apply(RuntimeControlInput::BeginRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        assert!(auth.is_running());
        assert!(
            auth.apply(RuntimeControlInput::BeginRun {
                run_id: RunId::new()
            })
            .is_err()
        );
    }

    // --- RunCompleted ---

    #[test]
    fn run_completed_returns_to_idle() {
        let (mut auth, run_id) = make_running_from_idle();
        let t = auth
            .apply(RuntimeControlInput::RunCompleted {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(auth.current_run_id().is_none());
        assert!(auth.pre_run_state().is_none());
    }

    #[test]
    fn run_completed_returns_to_attached() {
        let (mut auth, run_id) = make_running_from_attached();
        let t = auth
            .apply(RuntimeControlInput::RunCompleted {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Attached);
    }

    #[test]
    fn run_completed_returns_to_retired() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        let run_id = RunId::new();
        auth.apply(RuntimeControlInput::BeginRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        let t = auth
            .apply(RuntimeControlInput::RunCompleted {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Retired);
    }

    #[test]
    fn run_completed_rejected_wrong_run_id() {
        let (mut auth, _) = make_running_from_idle();
        assert!(
            auth.apply(RuntimeControlInput::RunCompleted {
                run_id: RunId::new()
            })
            .is_err()
        );
    }

    // --- RunFailed ---

    #[test]
    fn run_failed_returns_to_idle() {
        let (mut auth, run_id) = make_running_from_idle();
        let t = auth
            .apply(RuntimeControlInput::RunFailed {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
    }

    // --- RunCancelled ---

    #[test]
    fn run_cancelled_returns_to_attached() {
        let (mut auth, run_id) = make_running_from_attached();
        let t = auth
            .apply(RuntimeControlInput::RunCancelled {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Attached);
    }

    // --- SubmitWork ---

    #[test]
    fn submit_work_idle_emits_resolve_admission() {
        let mut auth = make_idle();
        let t = auth
            .apply(RuntimeControlInput::SubmitWork {
                work_id: "w1".into(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert_eq!(t.effects.len(), 1);
        assert!(matches!(
            &t.effects[0],
            RuntimeControlEffect::ResolveAdmission { work_id } if work_id == "w1"
        ));
    }

    #[test]
    fn submit_work_running_stays_running() {
        let (mut auth, _) = make_running_from_idle();
        let t = auth
            .apply(RuntimeControlInput::SubmitWork {
                work_id: "w1".into(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Running);
    }

    #[test]
    fn submit_work_attached_stays_attached() {
        let mut auth = make_attached();
        let t = auth
            .apply(RuntimeControlInput::SubmitWork {
                work_id: "w1".into(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Attached);
    }

    // --- AdmissionAccepted ---

    #[test]
    fn admission_accepted_idle_queue_signals_wake() {
        let mut auth = make_idle();
        let t = auth
            .apply(RuntimeControlInput::AdmissionAccepted {
                work_id: "w1".into(),
                handling_mode: HandlingMode::Queue,
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(auth.wake_pending());
        assert!(!auth.process_pending());
        assert!(t.effects.contains(&RuntimeControlEffect::SignalWake));
        assert!(
            !t.effects
                .contains(&RuntimeControlEffect::SignalImmediateProcess)
        );
    }

    #[test]
    fn admission_accepted_idle_steer_signals_wake_and_process() {
        let mut auth = make_idle();
        let t = auth
            .apply(RuntimeControlInput::AdmissionAccepted {
                work_id: "w1".into(),
                handling_mode: HandlingMode::Steer,
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(auth.wake_pending());
        assert!(auth.process_pending());
        assert!(t.effects.contains(&RuntimeControlEffect::SignalWake));
        assert!(
            t.effects
                .contains(&RuntimeControlEffect::SignalImmediateProcess)
        );
    }

    #[test]
    fn admission_accepted_running_queue_no_signals() {
        let (mut auth, _) = make_running_from_idle();
        let t = auth
            .apply(RuntimeControlInput::AdmissionAccepted {
                work_id: "w1".into(),
                handling_mode: HandlingMode::Queue,
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Running);
        assert!(!auth.wake_pending());
        assert!(!auth.process_pending());
        assert!(!t.effects.contains(&RuntimeControlEffect::SignalWake));
    }

    #[test]
    fn admission_accepted_running_steer_signals() {
        let (mut auth, _) = make_running_from_idle();
        let t = auth
            .apply(RuntimeControlInput::AdmissionAccepted {
                work_id: "w1".into(),
                handling_mode: HandlingMode::Steer,
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Running);
        assert!(auth.wake_pending());
        assert!(auth.process_pending());
        assert!(t.effects.contains(&RuntimeControlEffect::SignalWake));
        assert!(
            t.effects
                .contains(&RuntimeControlEffect::SignalImmediateProcess)
        );
    }

    #[test]
    fn admission_accepted_attached_queue_signals_wake() {
        let mut auth = make_attached();
        let t = auth
            .apply(RuntimeControlInput::AdmissionAccepted {
                work_id: "w1".into(),
                handling_mode: HandlingMode::Queue,
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Attached);
        assert!(auth.wake_pending());
        assert!(!auth.process_pending());
    }

    // --- AdmissionRejected ---

    #[test]
    fn admission_rejected_emits_notice() {
        let mut auth = make_idle();
        let t = auth
            .apply(RuntimeControlInput::AdmissionRejected {
                work_id: "w1".into(),
                reason: "quota".into(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(matches!(
            &t.effects[0],
            RuntimeControlEffect::EmitRuntimeNotice { kind, detail }
                if kind == "AdmissionRejected" && detail == "quota"
        ));
    }

    // --- AdmissionDeduplicated ---

    #[test]
    fn admission_deduplicated_emits_notice() {
        let mut auth = make_idle();
        let t = auth
            .apply(RuntimeControlInput::AdmissionDeduplicated {
                work_id: "w1".into(),
                existing_work_id: "w0".into(),
            })
            .unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(matches!(
            &t.effects[0],
            RuntimeControlEffect::EmitRuntimeNotice { kind, .. }
                if kind == "AdmissionDeduplicated"
        ));
    }

    // --- RecoverRequested ---

    #[test]
    fn recover_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::RecoverRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Recovering);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Idle));
    }

    #[test]
    fn recover_from_running_clears_run_id() {
        let (mut auth, _) = make_running_from_idle();
        let t = auth.apply(RuntimeControlInput::RecoverRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Recovering);
        assert!(auth.current_run_id().is_none());
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Running));
    }

    #[test]
    fn recover_from_attached() {
        let mut auth = make_attached();
        let t = auth.apply(RuntimeControlInput::RecoverRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Recovering);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Attached));
    }

    // --- RecoverySucceeded ---

    #[test]
    fn recovery_succeeded_resets_to_idle() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RecoverRequested).unwrap();
        let t = auth.apply(RuntimeControlInput::RecoverySucceeded).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(auth.current_run_id().is_none());
        assert!(auth.pre_run_state().is_none());
        assert!(!auth.wake_pending());
        assert!(!auth.process_pending());
    }

    // --- RetireRequested ---

    #[test]
    fn retire_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Retired);
        assert!(matches!(
            &t.effects[0],
            RuntimeControlEffect::ApplyControlPlaneCommand { command } if command == "Retire"
        ));
    }

    #[test]
    fn retire_from_attached() {
        let mut auth = make_attached();
        let t = auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Retired);
    }

    #[test]
    fn retire_from_running() {
        let (mut auth, _) = make_running_from_idle();
        let t = auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Retired);
    }

    // --- ResetRequested ---

    #[test]
    fn reset_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::ResetRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(t.effects.iter().any(|e| matches!(
            e,
            RuntimeControlEffect::ApplyControlPlaneCommand { command } if command == "Reset"
        )));
        assert!(t.effects.iter().any(|e| matches!(
            e,
            RuntimeControlEffect::ResolveCompletionAsTerminated { reason } if reason == "Reset"
        )));
    }

    #[test]
    fn reset_from_retired() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        let t = auth.apply(RuntimeControlInput::ResetRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
    }

    #[test]
    fn reset_rejected_from_running() {
        let (mut auth, _) = make_running_from_idle();
        assert!(auth.apply(RuntimeControlInput::ResetRequested).is_err());
    }

    // --- StopRequested ---

    #[test]
    fn stop_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::StopRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Stopped);
    }

    #[test]
    fn stop_from_running() {
        let (mut auth, _) = make_running_from_idle();
        let t = auth.apply(RuntimeControlInput::StopRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Stopped);
        assert!(auth.current_run_id().is_none());
    }

    #[test]
    fn stop_is_terminal() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::StopRequested).unwrap();
        assert!(auth.apply(RuntimeControlInput::ResetRequested).is_err());
    }

    // --- DestroyRequested ---

    #[test]
    fn destroy_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::DestroyRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Destroyed);
    }

    #[test]
    fn destroy_from_stopped() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::StopRequested).unwrap();
        let t = auth.apply(RuntimeControlInput::DestroyRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Destroyed);
    }

    #[test]
    fn destroy_is_terminal() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::DestroyRequested).unwrap();
        assert!(auth.apply(RuntimeControlInput::DestroyRequested).is_err());
    }

    // --- ResumeRequested ---

    #[test]
    fn resume_from_recovering() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RecoverRequested).unwrap();
        let t = auth.apply(RuntimeControlInput::ResumeRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
    }

    #[test]
    fn resume_rejected_from_idle() {
        let mut auth = make_idle();
        assert!(auth.apply(RuntimeControlInput::ResumeRequested).is_err());
    }

    // --- ExternalToolDeltaReceived ---

    #[test]
    fn external_tool_delta_stays_in_phase() {
        for init_phase in [
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
            RuntimeState::Recovering,
            RuntimeState::Retired,
        ] {
            let mut auth = RuntimeControlAuthority::from_state(init_phase);
            // For Running, we need a run_id to be valid
            if init_phase == RuntimeState::Running {
                auth.fields.current_run_id = Some(RunId::new());
            }
            let t = auth
                .apply(RuntimeControlInput::ExternalToolDeltaReceived)
                .unwrap();
            assert_eq!(
                t.next_phase, init_phase,
                "ExternalToolDelta should stay in {init_phase}"
            );
        }
    }

    // --- RecycleRequested ---

    #[test]
    fn recycle_from_retired() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        let t = auth.apply(RuntimeControlInput::RecycleRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Recovering);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Retired));
        assert!(t.effects.contains(&RuntimeControlEffect::InitiateRecycle));
    }

    #[test]
    fn recycle_from_idle() {
        let mut auth = make_idle();
        let t = auth.apply(RuntimeControlInput::RecycleRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Recovering);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Idle));
    }

    #[test]
    fn recycle_from_attached() {
        let mut auth = make_attached();
        let t = auth.apply(RuntimeControlInput::RecycleRequested).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Recovering);
        assert_eq!(auth.pre_run_state(), Some(RuntimeState::Attached));
    }

    #[test]
    fn recycle_rejected_with_active_run() {
        let (mut auth, _) = make_running_from_idle();
        // Complete the run first to get to idle, then manually set run_id
        // to simulate a bad state. Actually, Running is not in from list.
        assert!(auth.apply(RuntimeControlInput::RecycleRequested).is_err());
    }

    // --- RecycleSucceeded ---

    #[test]
    fn recycle_succeeded_from_recovering() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RecycleRequested).unwrap();
        let t = auth.apply(RuntimeControlInput::RecycleSucceeded).unwrap();
        assert_eq!(t.next_phase, RuntimeState::Idle);
        assert!(auth.current_run_id().is_none());
        assert!(auth.pre_run_state().is_none());
        assert!(matches!(
            &t.effects[0],
            RuntimeControlEffect::EmitRuntimeNotice { kind, detail }
                if kind == "Recycle" && detail == "Succeeded"
        ));
    }

    // --- Full cycles ---

    #[test]
    fn idle_running_idle_cycle() {
        let mut auth = make_idle();
        for _ in 0..3 {
            let run_id = RunId::new();
            auth.apply(RuntimeControlInput::BeginRun {
                run_id: run_id.clone(),
            })
            .unwrap();
            assert!(auth.is_running());
            auth.apply(RuntimeControlInput::RunCompleted {
                run_id: run_id.clone(),
            })
            .unwrap();
            assert!(auth.is_idle());
        }
    }

    #[test]
    fn attached_running_attached_cycle() {
        let mut auth = make_attached();
        for _ in 0..3 {
            let run_id = RunId::new();
            auth.apply(RuntimeControlInput::BeginRun {
                run_id: run_id.clone(),
            })
            .unwrap();
            assert!(auth.is_running());
            auth.apply(RuntimeControlInput::RunCompleted {
                run_id: run_id.clone(),
            })
            .unwrap();
            assert!(auth.is_attached());
        }
    }

    #[test]
    fn retired_drain_cycle() {
        let mut auth = make_idle();
        auth.apply(RuntimeControlInput::RetireRequested).unwrap();
        assert!(auth.can_process_queue());

        let run_id = RunId::new();
        auth.apply(RuntimeControlInput::BeginRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        assert!(auth.is_running());

        auth.apply(RuntimeControlInput::RunCompleted {
            run_id: run_id.clone(),
        })
        .unwrap();
        assert_eq!(auth.phase(), RuntimeState::Retired);
    }

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = make_idle();
        assert!(auth.can_accept(&RuntimeControlInput::AttachExecutor));
        assert!(!auth.can_accept(&RuntimeControlInput::DetachExecutor));
        assert_eq!(auth.phase(), RuntimeState::Idle);
    }

    // --- Destroy from all non-terminal phases ---

    #[test]
    fn destroy_from_all_non_terminal_phases() {
        for phase in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
            RuntimeState::Recovering,
            RuntimeState::Retired,
            RuntimeState::Stopped,
        ] {
            let mut auth = RuntimeControlAuthority::from_state(phase);
            let t = auth
                .apply(RuntimeControlInput::DestroyRequested)
                .unwrap_or_else(|_| panic!("destroy should work from {phase}"));
            assert_eq!(t.next_phase, RuntimeState::Destroyed);
        }
    }

    #[test]
    fn destroy_from_destroyed_is_rejected() {
        let mut auth = RuntimeControlAuthority::from_state(RuntimeState::Destroyed);
        assert!(auth.apply(RuntimeControlInput::DestroyRequested).is_err());
    }
}
