//! Typed vocabulary for owned tool processes that can outlive their host.
//!
//! Hosts that keep durable custody of the process groups their tools spawn
//! (shell calls, background jobs, monitors, command hooks) settle, at the
//! next start, every group an earlier host incarnation left behind. This
//! module owns the platform-independent facts of that settlement so the
//! runtime, the transcript and the host can all speak them without depending
//! on the custody implementation:
//!
//! - [`ToolProcessCessation`]: how an earlier incarnation's process was
//!   proven stopped.
//! - [`ToolProcessSpawner`]: what spawned it.
//! - [`InterruptedToolCall`] and [`InterruptedToolEvidence`]: durable evidence
//!   that such a process belonged to a run that was still in flight, so the
//!   run's inputs must be settled as interrupted instead of replayed.

use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::lifecycle::RunId;

/// How recovery established that an earlier incarnation's tool process has
/// ceased.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cessation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolProcessCessation {
    /// The host died before releasing the spawn gate, so the command never
    /// started.
    NeverStarted,
    /// The recorded group had no member left when recovery inspected it: the
    /// tool had already finished or been killed. Its result was not
    /// delivered.
    AlreadyExited,
    /// The recorded leader pid or group id now names a different process or
    /// group, so the recorded group is gone. Nothing was signalled.
    GroupReassigned,
    /// Recovery SIGKILLed the group and observed every member exit.
    KilledByRecovery { members: usize },
    /// The earlier incarnation ran in a boot or pid namespace that has since
    /// been replaced; every process of it has ended. Nothing was signalled.
    PriorEnvironmentEnded,
}

impl ToolProcessCessation {
    /// Whether the process may have run (and so may have had effects) before
    /// it ceased. Only a never-released spawn gate proves it did not.
    #[must_use]
    pub const fn may_have_run(self) -> bool {
        !matches!(self, Self::NeverStarted)
    }

    /// Human-readable description for model-facing projections.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::NeverStarted => "never started",
            Self::AlreadyExited => "had already exited; its result was lost",
            Self::GroupReassigned => "was already gone; its result was lost",
            Self::KilledByRecovery { .. } => "was still running and was terminated by recovery",
            Self::PriorEnvironmentEnded => {
                "ended with the host's previous boot or container; its result was lost"
            }
        }
    }
}

/// Which kind of owned process a custody entry guards.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolProcessSpawner {
    /// A foreground `shell` tool call.
    #[default]
    ShellCall,
    /// A background (detached) shell job attempt.
    BackgroundJob { job_id: String },
    /// A monitor job attempt.
    Monitor { job_id: String },
    /// A command hook invocation.
    CommandHook { hook_id: String },
}

impl ToolProcessSpawner {
    /// Human-readable description for model-facing projections.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::ShellCall => "shell tool call".to_owned(),
            Self::BackgroundJob { job_id } => format!("background shell job {job_id}"),
            Self::Monitor { job_id } => format!("monitor {job_id}"),
            Self::CommandHook { hook_id } => format!("command hook {hook_id}"),
        }
    }
}

/// Progress of one piece of interrupted-run evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptedToolSettlement {
    /// The run's in-flight inputs have not been settled yet.
    Pending,
    /// The run's in-flight inputs were settled as interrupted; the model has
    /// not been told yet.
    InputsSettled,
}

/// Durable evidence that an earlier host incarnation's tool process of this
/// session ceased while the run that started it was still in flight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedToolCall {
    /// Identity of the evidence (the custody entry).
    pub entry_id: Uuid,
    /// The run the tool process belonged to.
    pub run_id: RunId,
    /// Provider tool-call id of the interrupted call, when known.
    pub tool_call_id: Option<String>,
    pub spawner: ToolProcessSpawner,
    pub cessation: ToolProcessCessation,
    pub settlement: InterruptedToolSettlement,
}

/// Failure reading or advancing interrupted-run evidence.
#[derive(Debug, Clone, thiserror::Error)]
#[error("interrupted tool evidence failed: {reason}")]
pub struct InterruptedToolEvidenceError {
    pub reason: String,
}

/// Host-owned durable store of interrupted-run evidence for one session.
///
/// The runtime settles the evidence in order: settle the run's in-flight
/// inputs, [`Self::mark_inputs_settled`], deliver the typed notice to the
/// model, then [`Self::acknowledge`]. A crash between steps resumes from the
/// recorded settlement.
#[async_trait::async_trait]
pub trait InterruptedToolEvidence: Send + Sync {
    /// Every unacknowledged piece of evidence for the session.
    async fn interrupted_calls(
        &self,
    ) -> Result<Vec<InterruptedToolCall>, InterruptedToolEvidenceError>;

    /// Record that the in-flight inputs of these entries' runs are settled.
    async fn mark_inputs_settled(
        &self,
        entry_ids: &[Uuid],
    ) -> Result<(), InterruptedToolEvidenceError>;

    /// Discard these entries: fully settled, or moot (their run was not in
    /// flight).
    async fn acknowledge(&self, entry_ids: &[Uuid]) -> Result<(), InterruptedToolEvidenceError>;
}

/// Hand-off slot for a session's [`InterruptedToolEvidence`], carried by the
/// session's runtime bindings from agent construction (where the host
/// settles process custody) to runtime materialization (where the runtime
/// settles the interrupted run before serving).
#[derive(Default)]
pub struct InterruptedToolEvidenceSlot {
    evidence: Mutex<Option<Arc<dyn InterruptedToolEvidence>>>,
}

impl std::fmt::Debug for InterruptedToolEvidenceSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InterruptedToolEvidenceSlot")
            .field("installed", &self.installed())
            .finish()
    }
}

impl InterruptedToolEvidenceSlot {
    /// Install the session's evidence store (the latest installation wins).
    pub fn install(&self, evidence: Arc<dyn InterruptedToolEvidence>) {
        *self.evidence.lock().unwrap_or_else(PoisonError::into_inner) = Some(evidence);
    }

    /// The installed evidence store, if any.
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn InterruptedToolEvidence>> {
        self.evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn installed(&self) -> bool {
        self.evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}
