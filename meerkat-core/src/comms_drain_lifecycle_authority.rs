//! RMAT/MTAS sealed authority for the CommsDrainLifecycle machine.
//!
//! Owns spawn/stop/respawn lifecycle for the runtime-backed comms drain task.

use std::fmt;

/// Phase of the comms drain lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommsDrainPhase {
    Inactive,
    Starting,
    Running,
    ExitedRespawnable,
    Stopped,
}

impl fmt::Display for CommsDrainPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inactive => write!(f, "Inactive"),
            Self::Starting => write!(f, "Starting"),
            Self::Running => write!(f, "Running"),
            Self::ExitedRespawnable => write!(f, "ExitedRespawnable"),
            Self::Stopped => write!(f, "Stopped"),
        }
    }
}

/// Mode for the comms drain task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommsDrainMode {
    /// Long-lived host drain (no idle timeout, respawnable on failure).
    PersistentHost,
    /// Timed drain with idle timeout.
    Timed,
}

/// Reason the drain task exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainExitReason {
    IdleTimeout,
    Dismissed,
    Failed,
    Aborted,
    SessionShutdown,
}

/// Typed inputs for the CommsDrainLifecycle machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommsDrainLifecycleInput {
    EnsureRunning { mode: CommsDrainMode },
    TaskSpawned,
    TaskExited { reason: DrainExitReason },
    StopRequested,
    AbortObserved,
}

/// Effects emitted by CommsDrainLifecycle transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommsDrainLifecycleEffect {
    SpawnDrainTask { mode: CommsDrainMode },
    AbortDrainTask,
}

/// Successful transition outcome from the CommsDrainLifecycle authority.
#[derive(Debug)]
pub struct CommsDrainLifecycleTransition {
    pub from_phase: CommsDrainPhase,
    pub next_phase: CommsDrainPhase,
    pub effects: Vec<CommsDrainLifecycleEffect>,
}

/// Error returned when a transition is not legal from the current state.
#[derive(Debug, Clone)]
pub struct CommsDrainLifecycleError {
    pub from: CommsDrainPhase,
    pub input: String,
}

impl fmt::Display for CommsDrainLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "illegal comms drain lifecycle transition: {} in phase {}",
            self.input, self.from
        )
    }
}

impl std::error::Error for CommsDrainLifecycleError {}

#[derive(Debug, Clone)]
struct CommsDrainLifecycleFields {
    mode: Option<CommsDrainMode>,
}

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for CommsDrainLifecycle state mutation.
pub trait CommsDrainLifecycleMutator: sealed::Sealed {
    fn apply(
        &mut self,
        input: CommsDrainLifecycleInput,
    ) -> Result<CommsDrainLifecycleTransition, CommsDrainLifecycleError>;
}

/// Canonical authority for comms drain lifecycle state.
#[derive(Debug, Clone)]
pub struct CommsDrainLifecycleAuthority {
    phase: CommsDrainPhase,
    fields: CommsDrainLifecycleFields,
}

impl sealed::Sealed for CommsDrainLifecycleAuthority {}

impl CommsDrainLifecycleAuthority {
    pub fn new() -> Self {
        Self {
            phase: CommsDrainPhase::Inactive,
            fields: CommsDrainLifecycleFields { mode: None },
        }
    }

    pub fn phase(&self) -> CommsDrainPhase {
        self.phase
    }

    pub fn mode(&self) -> Option<CommsDrainMode> {
        self.fields.mode
    }

    fn reject(&self, input: &str) -> CommsDrainLifecycleError {
        CommsDrainLifecycleError {
            from: self.phase,
            input: input.to_string(),
        }
    }

    fn evaluate(
        &self,
        input: &CommsDrainLifecycleInput,
    ) -> Result<
        (
            CommsDrainPhase,
            CommsDrainLifecycleFields,
            Vec<CommsDrainLifecycleEffect>,
        ),
        CommsDrainLifecycleError,
    > {
        use CommsDrainLifecycleInput::{
            AbortObserved, EnsureRunning, StopRequested, TaskExited, TaskSpawned,
        };
        use CommsDrainPhase::{ExitedRespawnable, Inactive, Running, Starting, Stopped};

        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
            (Inactive, EnsureRunning { mode }) => {
                fields.mode = Some(*mode);
                effects.push(CommsDrainLifecycleEffect::SpawnDrainTask { mode: *mode });
                Starting
            }
            (Starting, TaskSpawned) => Running,
            (Starting | Running, TaskExited { reason }) => {
                if *reason == DrainExitReason::Failed
                    && fields.mode == Some(CommsDrainMode::PersistentHost)
                {
                    ExitedRespawnable
                } else {
                    Stopped
                }
            }
            (Running | Starting, StopRequested) => {
                effects.push(CommsDrainLifecycleEffect::AbortDrainTask);
                Stopped
            }
            (Running | Starting, AbortObserved) => Stopped,
            (ExitedRespawnable, EnsureRunning { mode }) => {
                fields.mode = Some(*mode);
                effects.push(CommsDrainLifecycleEffect::SpawnDrainTask { mode: *mode });
                Starting
            }
            (ExitedRespawnable, StopRequested) => Stopped,
            (Stopped, EnsureRunning { mode }) => {
                fields.mode = Some(*mode);
                effects.push(CommsDrainLifecycleEffect::SpawnDrainTask { mode: *mode });
                Starting
            }
            _ => {
                let input_name = match input {
                    EnsureRunning { .. } => "EnsureRunning",
                    TaskSpawned => "TaskSpawned",
                    TaskExited { .. } => "TaskExited",
                    StopRequested => "StopRequested",
                    AbortObserved => "AbortObserved",
                };
                return Err(self.reject(input_name));
            }
        };

        Ok((next_phase, fields, effects))
    }
}

impl CommsDrainLifecycleMutator for CommsDrainLifecycleAuthority {
    fn apply(
        &mut self,
        input: CommsDrainLifecycleInput,
    ) -> Result<CommsDrainLifecycleTransition, CommsDrainLifecycleError> {
        let from_phase = self.phase;
        let (next_phase, fields, effects) = self.evaluate(&input)?;
        self.phase = next_phase;
        self.fields = fields;
        Ok(CommsDrainLifecycleTransition {
            from_phase,
            next_phase,
            effects,
        })
    }
}

impl Default for CommsDrainLifecycleAuthority {
    fn default() -> Self {
        Self::new()
    }
}
