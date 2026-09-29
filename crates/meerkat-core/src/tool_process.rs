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
//!   that such a process belonged to a run that may still have been in
//!   flight, so the run's inputs must be settled as interrupted instead of
//!   replayed.
//! - [`InterruptedToolEvidenceSource`]: how a runtime obtains a session's
//!   evidence before it serves the session, whichever surface attaches it.
//! - [`InterruptedToolRunDisposition`]: what settling the evidence did to the
//!   run, as told to the model.

use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::lifecycle::RunId;
use crate::types::SessionId;

/// How recovery established that an earlier incarnation's tool process has
/// ceased.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
    /// The earlier incarnation ran in a boot that has since ended; every
    /// process of it has ended. Nothing was signalled.
    PriorEnvironmentEnded,
    /// The tool's process group exited and its owner proved every member
    /// gone, but the host stopped before the run that started it committed,
    /// so the run's result (including the tool's) was not committed.
    ExitedBeforeCommit,
    /// The earlier incarnation ran in another pid namespace of this boot (a
    /// container since restarted) and its host has exited, as proven by the
    /// kernel releasing its incarnation lock; its tools ended with that
    /// container. Nothing was signalled.
    ForeignIncarnationEnded,
    /// A cessation recorded by a newer version that this version does not
    /// recognize. The process has ceased; how is unknown here.
    #[serde(other)]
    Unknown,
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
                "ended with the host's previous boot; its result was lost"
            }
            Self::ExitedBeforeCommit => "had exited, but its run's result was not committed",
            Self::ForeignIncarnationEnded => {
                "ended with the host's previous container; its result was lost"
            }
            Self::Unknown => "has ceased; its result was lost",
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
    /// A spawner recorded by a newer version that this version does not
    /// recognize.
    #[serde(other)]
    Unknown,
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
            Self::Unknown => "tool process".to_owned(),
        }
    }
}

/// Progress of one piece of interrupted-run evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "settlement", rename_all = "snake_case")]
pub enum InterruptedToolSettlement {
    /// The run's in-flight inputs have not been settled yet.
    Pending,
    /// The run's in-flight inputs were settled as interrupted; the model has
    /// not been told yet.
    InputsSettled(InterruptedRunInputs),
}

impl InterruptedToolSettlement {
    /// The run's settled inputs, once settled.
    #[must_use]
    pub const fn settled_inputs(&self) -> Option<&InterruptedRunInputs> {
        match self {
            Self::Pending => None,
            Self::InputsSettled(inputs) => Some(inputs),
        }
    }
}

/// An interrupted run's in-flight inputs, as settled: the run's request plus
/// every input absorbed into it while it was in flight (steering), in
/// admission order.
///
/// The run never committed, so inputs that were never durably applied never
/// reached the transcript. The user requests among them are copied here
/// (captured before the inputs and their payloads were abandoned, bounded in
/// size, and deleted with the evidence) so the transcript can hold each of
/// them once, followed by the notice, instead of losing them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedRunInputs {
    pub inputs: Vec<InterruptedRunInput>,
}

impl InterruptedRunInputs {
    /// How many inputs the run had.
    #[must_use]
    pub fn count(&self) -> u32 {
        u32::try_from(self.inputs.len()).unwrap_or(u32::MAX)
    }

    /// The requests restored to the transcript, in admission order.
    pub fn requests(&self) -> impl Iterator<Item = &InterruptedRequest> {
        self.inputs
            .iter()
            .filter_map(|input| input.request.as_ref())
    }

    /// Kinds of the inputs whose content is not restored to the transcript
    /// (not a user request, already in the transcript, or over the size
    /// bound), in admission order.
    #[must_use]
    pub fn unrestored(&self) -> Vec<InterruptedInputKind> {
        self.inputs
            .iter()
            .filter(|input| input.request.is_none())
            .map(|input| input.kind)
            .collect()
    }
}

/// One in-flight input of an interrupted run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedRunInput {
    pub kind: InterruptedInputKind,
    /// The user request to restore to the transcript, when the input is a
    /// prompt that never reached the committed transcript and its content
    /// fits the evidence size bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<InterruptedRequest>,
}

/// A user request of an interrupted run, with the original transcript facts
/// it would have been committed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptedRequest {
    pub content: crate::types::ContentInput,
    /// When the request was admitted.
    pub created_at: crate::types::MessageTimestamp,
    /// The transcript identity the run would have stamped on it.
    #[serde(default)]
    pub identity: crate::types::TranscriptMessageIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<crate::types::RenderMetadata>,
}

/// What kind of input an interrupted run had in flight.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InterruptedInputKind {
    /// A user or operator prompt.
    Prompt,
    /// A peer (comms) message.
    Peer,
    /// A mob flow step.
    FlowStep,
    /// An external event.
    ExternalEvent,
    /// Runtime continuation work.
    Continuation,
    /// A non-content operation.
    Operation,
    /// A kind recorded by a newer version.
    #[serde(other)]
    Unknown,
}

impl InterruptedInputKind {
    /// Human-readable description for model-facing projections.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Prompt => "request",
            Self::Peer => "peer message",
            Self::FlowStep => "flow step",
            Self::ExternalEvent => "external event",
            Self::Continuation => "continuation",
            Self::Operation => "operation",
            Self::Unknown => "input",
        }
    }
}

/// What settling interrupted-run evidence did to the run the process belonged
/// to, as told to the model.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InterruptedToolRunDisposition {
    /// The run was still in flight. Its `inputs` (the request plus every
    /// input absorbed into the run while it ran) were settled as interrupted
    /// and not re-run. User requests among them are restored to the
    /// transcript right before the notice; `unrestored` names the kinds of
    /// the others (for example peer messages or flow steps), in order.
    InputsSettled {
        inputs: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unrestored: Vec<InterruptedInputKind>,
    },
    /// The run had already completed; only the process outlived it. Nothing
    /// was re-run or settled.
    RunCompleted,
    /// A disposition recorded by a newer version that this version does not
    /// recognize.
    #[serde(other)]
    Unknown,
}

/// Durable evidence that an earlier host incarnation's tool process of this
/// session ceased while the run that started it may still have been in
/// flight.
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
/// The runtime settles the evidence in order: [`Self::mark_inputs_settled`],
/// settle the run's in-flight inputs, deliver the typed notice to the model,
/// then [`Self::acknowledge`]. A crash between steps resumes from the
/// recorded settlement, and a notice delivered twice is a typed duplicate.
#[async_trait::async_trait]
pub trait InterruptedToolEvidence: Send + Sync {
    /// Every unacknowledged piece of evidence for the session.
    async fn interrupted_calls(
        &self,
    ) -> Result<Vec<InterruptedToolCall>, InterruptedToolEvidenceError>;

    /// Durably record that the in-flight inputs of these entries' run are
    /// settled, with the run's inputs as captured before they were
    /// abandoned.
    async fn mark_inputs_settled(
        &self,
        entry_ids: &[Uuid],
        inputs: &InterruptedRunInputs,
    ) -> Result<(), InterruptedToolEvidenceError>;

    /// Discard these entries: fully settled, or moot (their run was not in
    /// flight).
    async fn acknowledge(&self, entry_ids: &[Uuid]) -> Result<(), InterruptedToolEvidenceError>;

    /// The run reached a durable terminal in this host incarnation
    /// (committed, failed, cancelled or stopped): its inputs can no longer be
    /// replayed, so the host may discard the completed-tool markers it kept
    /// for the run. Best effort; a marker left behind is moot at the next
    /// recovery.
    async fn run_ended(&self, run_id: &RunId) -> Result<(), InterruptedToolEvidenceError>;
}

/// Host-side provider of interrupted-run evidence, installed on a runtime
/// so every attachment of a recovered session settles the session's
/// earlier-incarnation tool processes before it serves, even when no agent
/// has been built for the session yet.
#[async_trait::async_trait]
pub trait InterruptedToolEvidenceSource: Send + Sync {
    /// Settle the session's earlier-incarnation tool processes (proving each
    /// stopped, or stopping it) and return the session's evidence store.
    /// `Ok(None)` when the host keeps no process custody for the session.
    async fn settle_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Arc<dyn InterruptedToolEvidence>>, InterruptedToolEvidenceError>;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::SystemNoticeBlock;

    #[test]
    fn cessation_and_spawner_nest_under_their_own_kind_tag() {
        let block = SystemNoticeBlock::ToolProcessInterrupted {
            run_id: RunId::new(),
            tool_call_id: Some("call-1".to_owned()),
            spawner: ToolProcessSpawner::BackgroundJob {
                job_id: "job-1".to_owned(),
            },
            cessation: ToolProcessCessation::KilledByRecovery { members: 2 },
            disposition: InterruptedToolRunDisposition::InputsSettled {
                inputs: 2,
                unrestored: vec![InterruptedInputKind::Peer],
            },
        };
        let value = serde_json::to_value(&block).unwrap();
        assert_eq!(
            value["cessation"],
            serde_json::json!({ "kind": "killed_by_recovery", "members": 2 })
        );
        assert_eq!(
            value["spawner"],
            serde_json::json!({ "kind": "background_job", "job_id": "job-1" })
        );
        assert_eq!(
            value["disposition"],
            serde_json::json!({ "kind": "inputs_settled", "inputs": 2, "unrestored": ["peer"] })
        );
        let back: SystemNoticeBlock = serde_json::from_value(value).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn variants_from_a_newer_version_decode_as_unknown_inside_the_block() {
        let value = serde_json::json!({
            "type": "tool_process_interrupted",
            "run_id": RunId::new(),
            "spawner": { "kind": "future_spawner", "detail": 1 },
            "cessation": { "kind": "future_cessation" },
            "disposition": { "kind": "future_disposition" },
        });
        let block: SystemNoticeBlock = serde_json::from_value(value).unwrap();
        let SystemNoticeBlock::ToolProcessInterrupted {
            spawner,
            cessation,
            disposition,
            ..
        } = block
        else {
            panic!("expected the typed block, got {block:?}");
        };
        assert_eq!(spawner, ToolProcessSpawner::Unknown);
        assert_eq!(cessation, ToolProcessCessation::Unknown);
        assert_eq!(disposition, InterruptedToolRunDisposition::Unknown);
        assert!(cessation.may_have_run());
    }
}
