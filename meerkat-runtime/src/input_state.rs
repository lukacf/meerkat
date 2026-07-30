//! §13 InputState — per-input data shell.
//!
//! Canonical lifecycle truth for every input lives in the MeerkatMachine DSL
//! (`input_phases`, `input_run_associations`, `input_boundary_sequences` plus
//! the `QueueAccepted` / `StageForRun` / `RecordBoundarySeq` / etc.
//! transitions). This module owns ONLY the per-input shell metadata needed for
//! persistence/projection: a history log, timestamps, compatibility policy
//! snapshot, durability observation, idempotency key, and the cached payload
//! needed to rebuild queued work after recovery. Durability admission validity
//! and recovered keep/drop behavior are emitted by generated MeerkatMachine
//! inputs/effects.
//!
//! Terminal outcome and attempt count are DSL-owned facts. Live reads go
//! through `EphemeralRuntimeDriver::input_terminal_outcome` /
//! `input_attempt_count`; persistence carries them on [`InputStateSeed`].
//! `InputState` holds no copy of either.

use chrono::{DateTime, Utc};
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::InteractionId;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::{HandlingMode, SessionId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::identifiers::{InputKind, PolicyVersion};
use crate::ingress_types::RuntimeInputSemantics;
use crate::input::Input;
use crate::policy::PolicyDecision;

/// The lifecycle state of an input — mirrors the DSL's `input_phases` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputLifecycleState {
    Accepted,
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

/// Why an input was abandoned.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputAbandonReason {
    Retired,
    Reset,
    Stopped,
    Destroyed,
    Cancelled,
    MaxAttemptsExhausted { attempts: u32 },
}

/// Terminal outcome for an input.
///
/// The authoritative live copy is split across the DSL's typed terminal maps;
/// persistence carries it on [`InputStateSeed`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "outcome_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputTerminalOutcome {
    Consumed,
    Superseded { superseded_by: InputId },
    Coalesced { aggregate_id: InputId },
    Abandoned { reason: InputAbandonReason },
}

/// A single entry in the input's state history (shell bookkeeping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateHistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub from: InputLifecycleState,
    pub to: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Snapshot of the policy that was applied to this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub version: PolicyVersion,
    pub decision: PolicyDecision,
}

/// How a derived input can be reconstructed after crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReconstructionSource {
    Projection {
        rule_id: String,
        source_event_id: String,
    },
    Coalescing {
        source_input_ids: Vec<InputId>,
    },
}

/// Payload observed at the runtime boundary for one exact directed input.
/// Generated completion authority later classifies this candidate after the
/// runtime commit and session checkpoint have finalized.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "candidate_type", rename_all = "snake_case")]
pub(crate) enum InteractionTerminalCandidate {
    RunResult {
        result: Box<meerkat_core::types::RunResult>,
    },
    CompletedWithoutResult,
    CallbackPending {
        /// `None` on rows persisted by pre-durable-callback (v0.8.7) binaries,
        /// which wrote this variant without the field. The option is part of
        /// the persisted contract: a legacy row must re-serialize
        /// byte-identically so its stored `candidate_digest` keeps verifying.
        /// Live producers always write `Some`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        tool_name: String,
        args: serde_json::Value,
    },
    CallbackBatchPending {
        pending_tool_calls: Vec<meerkat_core::error::PendingCallbackToolCall>,
    },
    /// The runtime executor observed that the agent's generated turn machine
    /// had already reached a typed hard-failure terminal.  The metadata is
    /// durable because recovery must publish the same failure class and detail
    /// as the live completion path; display-text reclassification is forbidden.
    MachineTerminalFailure {
        error: meerkat_core::TurnErrorMetadata,
    },
    Cancelled,
    RuntimeTerminated {
        reason: String,
    },
}

impl InteractionTerminalCandidate {
    pub(crate) fn from_core_apply_terminal(
        terminal: Option<&meerkat_core::lifecycle::core_executor::CoreApplyTerminal>,
    ) -> Self {
        use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
        match terminal {
            Some(CoreApplyTerminal::RunResult(result)) => Self::RunResult {
                result: result.clone(),
            },
            Some(CoreApplyTerminal::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            }) => Self::CallbackPending {
                tool_use_id: Some(tool_use_id.clone()),
                tool_name: tool_name.clone(),
                args: args.clone(),
            },
            Some(CoreApplyTerminal::CallbackBatchPending { pending_tool_calls }) => {
                Self::CallbackBatchPending {
                    pending_tool_calls: pending_tool_calls.clone(),
                }
            }
            Some(CoreApplyTerminal::MachineTerminalFailure { error }) => {
                Self::MachineTerminalFailure {
                    error: error.clone(),
                }
            }
            Some(CoreApplyTerminal::NoPendingBoundary) | None => Self::CompletedWithoutResult,
        }
    }

    pub(crate) fn core_apply_terminal(
        &self,
    ) -> Option<meerkat_core::lifecycle::core_executor::CoreApplyTerminal> {
        use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
        match self {
            Self::RunResult { result } => Some(CoreApplyTerminal::RunResult(result.clone())),
            Self::CompletedWithoutResult => Some(CoreApplyTerminal::NoPendingBoundary),
            Self::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            } => Some(CoreApplyTerminal::CallbackPending {
                // A v0.8.7 row never recorded the id; empty means "identity
                // unknown, pre-0.8.8 row". Durable-callback consumers that
                // need the id never see such rows because the protocol did
                // not exist when they were written.
                tool_use_id: tool_use_id.clone().unwrap_or_default(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            }),
            Self::CallbackBatchPending { pending_tool_calls } => {
                Some(CoreApplyTerminal::CallbackBatchPending {
                    pending_tool_calls: pending_tool_calls.clone(),
                })
            }
            Self::MachineTerminalFailure { error } => {
                Some(CoreApplyTerminal::MachineTerminalFailure {
                    error: error.clone(),
                })
            }
            Self::Cancelled | Self::RuntimeTerminated { .. } => None,
        }
    }

    pub(crate) fn terminal_observation(
        &self,
    ) -> crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation {
        use crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation;
        match self {
            Self::RunResult { .. } => RuntimeCompletionTerminalObservation::RunResult,
            Self::CallbackPending { .. } | Self::CallbackBatchPending { .. } => {
                RuntimeCompletionTerminalObservation::CallbackPending
            }
            Self::RuntimeTerminated { .. } => {
                RuntimeCompletionTerminalObservation::RuntimeTerminated
            }
            Self::MachineTerminalFailure { .. } | Self::Cancelled => {
                RuntimeCompletionTerminalObservation::MachineTerminal
            }
            Self::CompletedWithoutResult => RuntimeCompletionTerminalObservation::NoResult,
        }
    }

    pub(crate) fn completion_error_metadata(&self) -> Option<meerkat_core::TurnErrorMetadata> {
        match self {
            Self::MachineTerminalFailure { error } => Some(error.clone()),
            _ => None,
        }
    }
}

/// Stable identity for one exact terminal-completion batch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub(crate) enum InputTerminalCompletionBatchKey {
    Run { run_id: RunId },
    RuntimeTermination { owner_input_id: InputId },
}

impl InputTerminalCompletionBatchKey {
    pub(crate) fn run_id(&self) -> Option<&RunId> {
        match self {
            Self::Run { run_id } => Some(run_id),
            Self::RuntimeTermination { .. } => None,
        }
    }
}

/// Machine-authorized verdict for the post-terminal finalization step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputTerminalCompletionFinalizationVerdict {
    Succeeded,
    Failed,
}

impl InputTerminalCompletionFinalizationVerdict {
    pub(crate) fn from_runtime_observation(
        observation: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    ) -> Self {
        match observation {
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded => {
                Self::Succeeded
            }
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed => {
                Self::Failed
            }
        }
    }

    pub(crate) fn runtime_observation(
        self,
    ) -> crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation {
        match self {
            Self::Succeeded => {
                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded
            }
            Self::Failed => {
                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed
            }
        }
    }
}

/// Durable phase of one exact input-completion batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum InputTerminalCompletionPhase {
    /// The terminal transaction retained the executor-observed candidate, but
    /// post-commit projection/checkpoint finalization has not selected the
    /// public completion class yet.
    Pending,
    /// Generated completion authority selected the exact public outcome. Only
    /// the canonical owner row carries the payload; peers bind it by digest.
    Finalized {
        receipt_digest: String,
        finalization: InputTerminalCompletionFinalizationVerdict,
    },
}

/// Durable exact-completion carrier attached to every terminal input row.
///
/// The potentially large candidate/outcome payload is stored on exactly one
/// canonical owner row. Other rows carry only immutable batch identity and
/// digests, keeping multi-input turns O(one result + batch cardinality).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InputTerminalCompletion {
    pub(crate) input_id: InputId,
    pub(crate) batch_ordinal: u16,
    pub(crate) batch_key: InputTerminalCompletionBatchKey,
    pub(crate) owner_input_id: InputId,
    pub(crate) candidate_digest: String,
    pub(crate) completion_input_ids_digest: String,
    pub(crate) requires_session_checkpoint: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) candidate: Option<InteractionTerminalCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completion_input_ids: Option<Vec<InputId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<crate::completion::CompletionOutcome>,
    pub(crate) phase: InputTerminalCompletionPhase,
}

impl InputTerminalCompletion {
    pub(crate) fn validate_row(&self) -> Result<(), String> {
        if self.candidate_digest.is_empty() || self.completion_input_ids_digest.is_empty() {
            return Err("terminal completion row carried an empty immutable digest".into());
        }
        let owns_payload = self.input_id == self.owner_input_id;
        if owns_payload != (self.batch_ordinal == 0) {
            return Err("terminal completion owner/ordinal mismatch".into());
        }
        if let InputTerminalCompletionBatchKey::RuntimeTermination { owner_input_id } =
            &self.batch_key
            && owner_input_id != &self.owner_input_id
        {
            return Err("runless terminal completion key/owner mismatch".into());
        }
        if matches!(
            &self.batch_key,
            InputTerminalCompletionBatchKey::RuntimeTermination { .. }
        ) && self.requires_session_checkpoint
        {
            return Err("runless terminal completion unexpectedly requires a checkpoint".into());
        }
        match (&self.completion_input_ids, owns_payload) {
            (Some(input_ids), true) => {
                if input_ids.is_empty() || input_ids.len() > 256 {
                    return Err("terminal completion recipient set has invalid size".into());
                }
                if input_ids
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != input_ids.len()
                {
                    return Err("terminal completion recipient set contains duplicates".into());
                }
                if input_ids
                    .windows(2)
                    .any(|window| window[0].0 >= window[1].0)
                {
                    return Err(
                        "terminal completion recipient set is not in canonical order".into(),
                    );
                }
                if input_ids.first() != Some(&self.owner_input_id) {
                    return Err("terminal completion recipient set lost its canonical owner".into());
                }
                if interaction_terminal_payload_digest(input_ids)?
                    != self.completion_input_ids_digest
                {
                    return Err("terminal completion recipient digest mismatch".into());
                }
            }
            (None, false) => {}
            _ => return Err("terminal completion payload ownership is invalid".into()),
        }
        match (&self.phase, &self.candidate, &self.outcome, owns_payload) {
            (InputTerminalCompletionPhase::Pending, Some(candidate), None, true) => {
                if interaction_terminal_payload_digest(candidate)? != self.candidate_digest {
                    return Err("terminal completion candidate digest mismatch".into());
                }
                match (&self.batch_key, candidate) {
                    (
                        InputTerminalCompletionBatchKey::RuntimeTermination { .. },
                        InteractionTerminalCandidate::RuntimeTerminated { .. },
                    ) => {}
                    (
                        InputTerminalCompletionBatchKey::Run { .. },
                        InteractionTerminalCandidate::RuntimeTerminated { .. },
                    )
                    | (InputTerminalCompletionBatchKey::RuntimeTermination { .. }, _) => {
                        return Err("terminal completion scope does not match its candidate".into());
                    }
                    (InputTerminalCompletionBatchKey::Run { .. }, _) => {}
                }
            }
            (InputTerminalCompletionPhase::Pending, None, None, false) => {}
            (
                InputTerminalCompletionPhase::Finalized {
                    receipt_digest,
                    finalization,
                },
                None,
                Some(outcome),
                true,
            ) => {
                if interaction_terminal_payload_digest(&(outcome, finalization))? != *receipt_digest
                {
                    return Err("terminal completion receipt digest mismatch".into());
                }
                match (finalization, outcome) {
                    (
                        InputTerminalCompletionFinalizationVerdict::Failed,
                        crate::completion::CompletionOutcome::CompletedWithFinalizationFailure {
                            ..
                        }
                        | crate::completion::CompletionOutcome::AbandonedWithError { .. },
                    ) => {}
                    (
                        InputTerminalCompletionFinalizationVerdict::Succeeded,
                        crate::completion::CompletionOutcome::CompletedWithFinalizationFailure {
                            ..
                        },
                    )
                    | (InputTerminalCompletionFinalizationVerdict::Failed, _) => {
                        return Err(
                            "terminal completion outcome contradicts its finalization verdict"
                                .into(),
                        );
                    }
                    (InputTerminalCompletionFinalizationVerdict::Succeeded, _) => {}
                }
            }
            (InputTerminalCompletionPhase::Finalized { .. }, None, None, false) => {}
            _ => return Err("terminal completion phase/payload shape is invalid".into()),
        }
        Ok(())
    }
}

/// Validate one complete terminal-completion batch and return its canonical
/// owner row. Callers must supply every row in ordinal order.
pub(crate) fn validate_input_terminal_completion_batch(
    rows: &[InputTerminalCompletion],
) -> Result<&InputTerminalCompletion, String> {
    if rows.is_empty() || rows.len() > 256 {
        return Err("terminal completion batch has invalid size".into());
    }
    let owner = &rows[0];
    let owner_input_ids = owner
        .completion_input_ids
        .as_ref()
        .ok_or_else(|| "terminal completion batch owner lost recipient set".to_string())?;
    if owner_input_ids.len() != rows.len() {
        return Err("terminal completion batch row/recipient cardinality mismatch".into());
    }
    for (ordinal, row) in rows.iter().enumerate() {
        row.validate_row()?;
        if usize::from(row.batch_ordinal) != ordinal
            || row.input_id != owner_input_ids[ordinal]
            || row.batch_key != owner.batch_key
            || row.owner_input_id != owner.owner_input_id
            || row.candidate_digest != owner.candidate_digest
            || row.completion_input_ids_digest != owner.completion_input_ids_digest
            || row.requires_session_checkpoint != owner.requires_session_checkpoint
        {
            return Err("terminal completion batch has split immutable identity".into());
        }
        match (&owner.phase, &row.phase) {
            (InputTerminalCompletionPhase::Pending, InputTerminalCompletionPhase::Pending) => {}
            (
                InputTerminalCompletionPhase::Finalized {
                    receipt_digest: owner_digest,
                    finalization: owner_finalization,
                },
                InputTerminalCompletionPhase::Finalized {
                    receipt_digest,
                    finalization,
                },
            ) if receipt_digest == owner_digest && finalization == owner_finalization => {}
            _ => return Err("terminal completion batch has split phase".into()),
        }
    }
    Ok(owner)
}

/// Durable receipt proving that the exact interaction terminal row was
/// appended (or byte-identically replayed) in the session event store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InteractionTerminalPublication {
    pub(crate) terminal_seq: u64,
    pub(crate) payload_digest: String,
}

/// Structurally valid durable phases for one directed-terminal outbox row.
///
/// The batch candidate remains on exactly one owner row until publication.
/// Once an exact event-store receipt is durable, both the candidate and the
/// finalized event are compacted away; their immutable digests and the exact
/// terminal sequence remain as the retry/provenance witness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum InteractionTerminalOutboxPhase {
    Candidate,
    Finalized {
        finalization_failed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finalized_event: Option<AgentEvent>,
        finalized_payload_digest: String,
    },
    Published {
        finalization_failed: bool,
        publication: InteractionTerminalPublication,
    },
}

/// Typed durable identity for one exact terminal batch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub(crate) enum InteractionTerminalBatchKey {
    Run { run_id: RunId },
    RuntimeTermination { candidate_owner_input_id: InputId },
}

impl InteractionTerminalBatchKey {
    pub(crate) fn run_id(&self) -> Option<&RunId> {
        match self {
            Self::Run { run_id } => Some(run_id),
            Self::RuntimeTermination { .. } => None,
        }
    }
}

/// Retry carrier for an exact per-input terminal publication.
///
/// This is shell payload, not a competing lifecycle machine: candidate
/// creation is bound to the machine-owned run commit/failure path, final event
/// creation consumes generated runtime-completion authority, and publication
/// is accepted only with an exact event-store receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InteractionTerminalOutbox {
    pub(crate) interaction_id: InteractionId,
    pub(crate) input_id: InputId,
    /// Stable position in the directed event batch. Ordinals are contiguous
    /// from zero and the shared candidate owner is always ordinal zero.
    pub(crate) batch_ordinal: u16,
    pub(crate) batch_key: InteractionTerminalBatchKey,
    pub(crate) owner_session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_agent_runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_fence_token: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_runtime_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_runtime_epoch_id: Option<String>,
    /// Input row that owns the batch's single shared candidate payload.
    pub(crate) candidate_owner_input_id: InputId,
    /// Shared candidate payload, present only on the owner row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) candidate: Option<InteractionTerminalCandidate>,
    pub(crate) candidate_digest: String,
    /// Exact completion-waiter recipients for the whole runtime batch. The
    /// vector is stored only on the candidate owner and compacted after the
    /// terminal publication receipt is durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completion_input_ids: Option<Vec<InputId>>,
    /// Shared immutable proof for the owner-only recipient vector.
    pub(crate) completion_input_ids_digest: String,
    pub(crate) phase: InteractionTerminalOutboxPhase,
}

pub(crate) fn interaction_terminal_payload_digest<T: Serialize>(
    payload: &T,
) -> Result<String, String> {
    serde_json::to_vec(payload)
        .map(|encoded| format!("{:x}", Sha256::digest(encoded)))
        .map_err(|error| format!("failed to encode interaction terminal payload: {error}"))
}

pub(crate) fn interaction_terminal_event_id(event: &AgentEvent) -> Option<InteractionId> {
    match event {
        AgentEvent::InteractionComplete { interaction_id, .. }
        | AgentEvent::InteractionCallbackPending { interaction_id, .. }
        | AgentEvent::InteractionFailed { interaction_id, .. } => Some(*interaction_id),
        _ => None,
    }
}

pub(crate) fn interaction_terminal_event_for_id(
    event: &AgentEvent,
    interaction_id: InteractionId,
) -> Option<AgentEvent> {
    match event {
        AgentEvent::InteractionComplete {
            result,
            structured_output,
            ..
        } => Some(AgentEvent::InteractionComplete {
            interaction_id,
            result: result.clone(),
            structured_output: structured_output.clone(),
        }),
        AgentEvent::InteractionCallbackPending {
            tool_name,
            args,
            pending_tool_calls,
            ..
        } => Some(AgentEvent::InteractionCallbackPending {
            interaction_id,
            tool_name: tool_name.clone(),
            args: args.clone(),
            pending_tool_calls: pending_tool_calls.clone(),
        }),
        AgentEvent::InteractionFailed { reason, .. } => Some(AgentEvent::InteractionFailed {
            interaction_id,
            reason: reason.clone(),
        }),
        _ => None,
    }
}

impl InteractionTerminalOutbox {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.input_id.0 != self.interaction_id.0 {
            return Err("interaction terminal outbox input/interaction identity mismatch".into());
        }
        if self.candidate_digest.is_empty() {
            return Err("interaction terminal outbox candidate digest is empty".into());
        }
        if self.completion_input_ids_digest.is_empty() {
            return Err("interaction terminal outbox completion recipient digest is empty".into());
        }
        if self.owner_agent_runtime_id.is_none()
            || self.owner_fence_token.is_none()
            || self.owner_runtime_generation.is_none()
        {
            return Err(
                "interaction terminal outbox is missing required runtime placement binding".into(),
            );
        }
        match (&self.batch_key, self.candidate.as_ref()) {
            (
                InteractionTerminalBatchKey::RuntimeTermination {
                    candidate_owner_input_id,
                },
                candidate,
            ) => {
                if candidate_owner_input_id != &self.candidate_owner_input_id {
                    return Err("runtime-termination batch key/candidate-owner mismatch".into());
                }
                if candidate.is_some_and(|candidate| {
                    !matches!(
                        candidate,
                        InteractionTerminalCandidate::RuntimeTerminated { .. }
                    )
                }) {
                    return Err("runtime-termination batch carried a run-scoped candidate".into());
                }
            }
            (
                InteractionTerminalBatchKey::Run { .. },
                Some(InteractionTerminalCandidate::RuntimeTerminated { .. }),
            ) => {
                return Err("run-scoped terminal batch carried runtime termination".into());
            }
            (InteractionTerminalBatchKey::Run { .. }, _) => {}
        }
        let published = matches!(self.phase, InteractionTerminalOutboxPhase::Published { .. });
        let owns_candidate = self.input_id == self.candidate_owner_input_id;
        if owns_candidate != (self.batch_ordinal == 0) {
            return Err("interaction terminal outbox candidate owner/ordinal mismatch".into());
        }
        match (&self.completion_input_ids, owns_candidate, published) {
            (Some(input_ids), true, false) => {
                if input_ids.is_empty() || input_ids.len() > 256 {
                    return Err(
                        "interaction terminal completion recipient set has invalid size".into(),
                    );
                }
                let unique = input_ids.iter().collect::<std::collections::HashSet<_>>();
                if unique.len() != input_ids.len() {
                    return Err(
                        "interaction terminal completion recipient set contains duplicates".into(),
                    );
                }
                if !input_ids.contains(&self.input_id) {
                    return Err(
                        "interaction terminal candidate owner is not a completion recipient".into(),
                    );
                }
                if interaction_terminal_payload_digest(input_ids)?
                    != self.completion_input_ids_digest
                {
                    return Err("interaction terminal completion recipient digest mismatch".into());
                }
            }
            (None, false, false) | (None, _, true) => {}
            (Some(_), false, _) => {
                return Err("non-owner interaction outbox duplicated completion recipients".into());
            }
            (Some(_), true, true) => {
                return Err("published interaction outbox retained completion recipients".into());
            }
            (None, true, false) => {
                return Err("interaction outbox candidate owner lost completion recipients".into());
            }
        }
        match (&self.candidate, owns_candidate, published) {
            (Some(candidate), true, false) => {
                if interaction_terminal_payload_digest(candidate)? != self.candidate_digest {
                    return Err("interaction terminal outbox candidate digest mismatch".into());
                }
                if let InteractionTerminalCandidate::RunResult { result } = candidate
                    && result.session_id != self.owner_session_id
                {
                    return Err("interaction terminal candidate session/owner mismatch".into());
                }
            }
            (None, false, false) => {}
            (None, _, true) => {}
            (Some(_), false, _) => {
                return Err("non-owner interaction outbox duplicated shared candidate".into());
            }
            (Some(_), true, true) => {
                return Err("published interaction outbox retained its shared candidate".into());
            }
            (None, true, false) => {
                return Err("interaction outbox candidate owner has no candidate".into());
            }
        }
        match &self.phase {
            InteractionTerminalOutboxPhase::Candidate => {}
            InteractionTerminalOutboxPhase::Finalized {
                finalization_failed,
                finalized_event: Some(event),
                finalized_payload_digest: digest,
            } if self.input_id == self.candidate_owner_input_id => {
                if interaction_terminal_event_id(event) != Some(self.interaction_id) {
                    return Err(
                        "interaction terminal outbox finalized event identity mismatch".into(),
                    );
                }
                if interaction_terminal_payload_digest(event)? != *digest {
                    return Err("interaction terminal outbox finalized digest mismatch".into());
                }
                let Some(candidate) = self.candidate.as_ref() else {
                    return Err("finalized candidate owner lost shared candidate".into());
                };
                if !interaction_terminal_candidate_matches_event(
                    candidate,
                    self.interaction_id,
                    event,
                    *finalization_failed,
                ) {
                    return Err("interaction terminal finalized event/candidate mismatch".into());
                }
            }
            InteractionTerminalOutboxPhase::Finalized {
                finalized_event: None,
                finalized_payload_digest,
                ..
            } if self.input_id != self.candidate_owner_input_id
                && !finalized_payload_digest.is_empty() => {}
            InteractionTerminalOutboxPhase::Finalized { .. } => {
                return Err(
                    "interaction terminal outbox finalized payload ownership is invalid".into(),
                );
            }
            InteractionTerminalOutboxPhase::Published { publication, .. } => {
                if publication.terminal_seq == 0 {
                    return Err("interaction terminal publication sequence must be non-zero".into());
                }
                if publication.payload_digest.is_empty() {
                    return Err("interaction terminal publication digest is empty".into());
                }
            }
        }
        Ok(())
    }
}

/// Validate immutable cross-row identity and ordering for one exact terminal
/// batch. Callers must order rows by `batch_ordinal` first.
pub(crate) fn validate_interaction_terminal_outbox_batch_shape(
    outboxes: &[InteractionTerminalOutbox],
) -> Result<(), String> {
    if outboxes.is_empty() || outboxes.len() > 256 {
        return Err("interaction terminal batch has invalid directed-row count".into());
    }
    let owner = &outboxes[0];
    if owner.batch_ordinal != 0 || owner.input_id != owner.candidate_owner_input_id {
        return Err("interaction terminal batch has no ordinal-zero candidate owner".into());
    }
    let mut row_input_ids = std::collections::HashSet::new();
    for (ordinal, outbox) in outboxes.iter().enumerate() {
        outbox.validate()?;
        if usize::from(outbox.batch_ordinal) != ordinal {
            return Err("interaction terminal batch ordinals are not contiguous".into());
        }
        if outbox.batch_key != owner.batch_key
            || outbox.candidate_owner_input_id != owner.candidate_owner_input_id
            || outbox.candidate_digest != owner.candidate_digest
            || outbox.completion_input_ids_digest != owner.completion_input_ids_digest
        {
            return Err("interaction terminal batch has split immutable identity".into());
        }
        if !row_input_ids.insert(outbox.input_id.clone()) {
            return Err("interaction terminal batch repeats a directed input".into());
        }
    }
    Ok(())
}

/// Validate the cross-row invariants of one unpublished exact terminal batch.
/// Callers must order rows by `batch_ordinal` before invoking this helper.
pub(crate) fn validate_unpublished_interaction_terminal_outbox_batch(
    outboxes: &[InteractionTerminalOutbox],
) -> Result<Vec<InputId>, String> {
    validate_interaction_terminal_outbox_batch_shape(outboxes)?;
    let owner = &outboxes[0];
    let completion_input_ids = owner.completion_input_ids.clone().ok_or_else(|| {
        "unpublished interaction terminal batch owner lost completion recipients".to_string()
    })?;
    for outbox in outboxes {
        if matches!(
            outbox.phase,
            InteractionTerminalOutboxPhase::Published { .. }
        ) {
            return Err("published row appeared in an unpublished terminal batch".into());
        }
        if !completion_input_ids.contains(&outbox.input_id) {
            return Err("directed terminal input is not a completion recipient".into());
        }
    }
    Ok(completion_input_ids)
}

pub(crate) fn interaction_terminal_candidate_matches_event(
    candidate: &InteractionTerminalCandidate,
    interaction_id: InteractionId,
    event: &AgentEvent,
    finalization_failed: bool,
) -> bool {
    use meerkat_core::event::InteractionFailureReason;
    if interaction_terminal_event_id(event) != Some(interaction_id) {
        return false;
    }
    if finalization_failed {
        return matches!(
            (candidate, event),
            (
                InteractionTerminalCandidate::RunResult { .. },
                AgentEvent::InteractionFailed {
                    reason: InteractionFailureReason::FinalizationFailed { .. },
                    ..
                },
            ) | (
                InteractionTerminalCandidate::CompletedWithoutResult
                    | InteractionTerminalCandidate::CallbackPending { .. }
                    | InteractionTerminalCandidate::CallbackBatchPending { .. }
                    | InteractionTerminalCandidate::MachineTerminalFailure { .. },
                AgentEvent::InteractionFailed {
                    reason: InteractionFailureReason::Abandoned { .. },
                    ..
                },
            )
        );
    }
    match (candidate, event) {
        (
            InteractionTerminalCandidate::RunResult { result },
            AgentEvent::InteractionComplete {
                result: event_result,
                structured_output: event_structured,
                ..
            },
        ) if result.extraction_error.is_none() => {
            result.text == *event_result && result.structured_output == *event_structured
        }
        (
            InteractionTerminalCandidate::RunResult { result },
            AgentEvent::InteractionFailed {
                reason:
                    InteractionFailureReason::ExtractionFailed {
                        last_output,
                        attempts,
                        reason,
                    },
                ..
            },
        ) if result.extraction_error.is_some() => {
            let Some(extraction) = result.extraction_error.as_ref() else {
                return false;
            };
            extraction.last_output == *last_output
                && extraction.attempts == *attempts
                && extraction.reason == *reason
        }
        (
            InteractionTerminalCandidate::CompletedWithoutResult,
            AgentEvent::InteractionComplete {
                result,
                structured_output,
                ..
            },
        ) => result.is_empty() && structured_output.is_none(),
        (
            InteractionTerminalCandidate::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            },
            AgentEvent::InteractionCallbackPending {
                tool_name: event_tool,
                args: event_args,
                pending_tool_calls,
                ..
            },
        ) => {
            tool_name == event_tool
                && args == event_args
                && match tool_use_id {
                    Some(tool_use_id) => {
                        pending_tool_calls.as_slice()
                            == [meerkat_core::error::PendingCallbackToolCall {
                                tool_use_id: tool_use_id.clone(),
                                tool_name: tool_name.clone(),
                                args: args.clone(),
                            }]
                    }
                    // A v0.8.7 candidate pairs with a v0.8.7 finalized event
                    // (no pending set) or with an event this binary finalized
                    // from the same legacy candidate (unknown-identity id).
                    None => {
                        pending_tool_calls.is_empty()
                            || pending_tool_calls.as_slice()
                                == [meerkat_core::error::PendingCallbackToolCall {
                                    tool_use_id: String::new(),
                                    tool_name: tool_name.clone(),
                                    args: args.clone(),
                                }]
                    }
                }
        }
        (
            InteractionTerminalCandidate::CallbackBatchPending { pending_tool_calls },
            AgentEvent::InteractionCallbackPending {
                pending_tool_calls: event_pending,
                ..
            },
        ) => pending_tool_calls == event_pending,
        (
            InteractionTerminalCandidate::MachineTerminalFailure { error },
            AgentEvent::InteractionFailed {
                reason: InteractionFailureReason::Abandoned { detail },
                ..
            },
        ) => error.detail.as_deref() == Some(detail.as_str()),
        (
            InteractionTerminalCandidate::Cancelled,
            AgentEvent::InteractionFailed {
                reason: InteractionFailureReason::Cancelled,
                ..
            },
        ) => true,
        (
            InteractionTerminalCandidate::RuntimeTerminated { reason },
            AgentEvent::InteractionFailed {
                reason: InteractionFailureReason::Abandoned { detail },
                ..
            },
        ) => reason == detail,
        _ => false,
    }
}

/// An event on an input's state (for event sourcing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateEvent {
    pub timestamp: DateTime<Utc>,
    pub state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// DSL-owned lifecycle projection for an input.
///
/// Carries the fields that are authoritative in the MeerkatMachine DSL
/// (`input_phases`, `input_run_associations`, `input_boundary_sequences`,
/// `input_terminal_kind` + `input_superseded_by` / `input_aggregate_id` /
/// `input_abandon_reason` / `input_abandon_attempt_count`, and
/// `input_attempt_counts` / `input_admission_seq` / `input_recovery_lanes`) so
/// they can travel alongside a persisted [`InputState`] at the store boundary,
/// where no live DSL is available to query. Inside a running driver, these
/// values are always read from the DSL directly, never from the seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputStateSeed {
    pub phase: InputLifecycleState,
    pub last_run_id: Option<RunId>,
    pub last_boundary_sequence: Option<u64>,
    pub admission_sequence: Option<u64>,
    pub terminal_outcome: Option<InputTerminalOutcome>,
    pub attempt_count: u32,
    pub recovery_lane: Option<HandlingMode>,
}

impl InputStateSeed {
    /// Freshly-accepted input: no run association, no boundary sequence,
    /// no terminal outcome, zero attempts.
    pub fn new_accepted() -> Self {
        Self {
            phase: InputLifecycleState::Accepted,
            last_run_id: None,
            last_boundary_sequence: None,
            admission_sequence: None,
            terminal_outcome: None,
            attempt_count: 0,
            recovery_lane: None,
        }
    }
}

/// Persisted bundle: shell [`InputState`] plus its [`InputStateSeed`].
///
/// Used at the store boundary so the DSL-owned fields survive persistence
/// without being re-shadowed onto `InputState` itself. Recovery treats the
/// seed as a durable witness and re-enters the recovered facts through typed
/// machine inputs; it does not hydrate DSL state directly from this bundle.
#[derive(Debug, Clone)]
pub struct StoredInputState {
    pub state: InputState,
    pub seed: InputStateSeed,
}

/// Runtime-issued proof that an exact directed interaction terminal was
/// durably published. The private fields prevent consumers from fabricating
/// proof out of public input-shell metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedDirectedTerminalBinding {
    input_id: InputId,
    interaction_id: InteractionId,
}

impl PublishedDirectedTerminalBinding {
    pub fn binds(&self, input_id: &InputId, interaction_id: InteractionId) -> bool {
        &self.input_id == input_id && self.interaction_id == interaction_id
    }
}

impl StoredInputState {
    /// Convenience: freshly-accepted bundle.
    pub fn new_accepted(input_id: InputId) -> Self {
        Self {
            state: InputState::new_accepted(input_id),
            seed: InputStateSeed::new_accepted(),
        }
    }

    /// Return a sealed compact binding only after the runtime-private terminal
    /// outbox validates and carries its durable publication receipt.
    pub fn published_directed_terminal_binding(
        &self,
    ) -> Result<Option<PublishedDirectedTerminalBinding>, String> {
        let Some(outbox) = self.state.interaction_terminal_outbox.as_ref() else {
            return Ok(None);
        };
        outbox.validate()?;
        if !matches!(
            outbox.phase,
            InteractionTerminalOutboxPhase::Published { .. }
        ) {
            return Ok(None);
        }
        Ok(Some(PublishedDirectedTerminalBinding {
            input_id: outbox.input_id.clone(),
            interaction_id: outbox.interaction_id,
        }))
    }
}

/// Resolve one exact public completion from a full runtime input snapshot.
///
/// `Ok(None)` means no finalized receipt exists. Any partial batch, digest
/// mismatch, or owner loss is corruption rather than absence.
pub(crate) fn input_terminal_completion_outcome(
    states: &[StoredInputState],
    input_id: &InputId,
) -> Result<Option<crate::completion::CompletionOutcome>, InputTerminalCompletionReadError> {
    let Some(stored) = states
        .iter()
        .find(|stored| &stored.state.input_id == input_id)
    else {
        return Ok(None);
    };
    let Some(target) = stored.state.terminal_completion.as_ref() else {
        if stored.seed.terminal_outcome.is_some() {
            return if stored.state.terminal_completion_unavailable {
                Err(InputTerminalCompletionReadError::MigratedReceiptUnavailable)
            } else {
                Err(InputTerminalCompletionReadError::Corrupt(
                    "v5 terminal input lost its exact completion receipt".to_string(),
                ))
            };
        }
        return Ok(None);
    };
    if states.iter().any(|stored| {
        stored
            .state
            .terminal_completion
            .as_ref()
            .is_some_and(|completion| {
                completion.input_id != stored.state.input_id
                    || stored.seed.terminal_outcome.is_none()
            })
    }) {
        return Err(InputTerminalCompletionReadError::Corrupt(
            "terminal completion row is not bound to the same terminal input state".to_string(),
        ));
    }
    let mut rows = states
        .iter()
        .filter_map(|stored| stored.state.terminal_completion.clone())
        .filter(|row| row.batch_key == target.batch_key)
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.batch_ordinal);
    let owner = validate_input_terminal_completion_batch(&rows)
        .map_err(InputTerminalCompletionReadError::Corrupt)?;
    match &owner.phase {
        InputTerminalCompletionPhase::Pending => Ok(None),
        InputTerminalCompletionPhase::Finalized { .. } => {
            owner.outcome.clone().map(Some).ok_or_else(|| {
                InputTerminalCompletionReadError::Corrupt(
                    "finalized terminal completion owner lost outcome".to_string(),
                )
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum InputTerminalCompletionReadError {
    #[error(
        "0.8.10 terminal input predates exact completion receipts; its public outcome cannot be reconstructed"
    )]
    MigratedReceiptUnavailable,
    #[error("{0}")]
    Corrupt(String),
}

/// Store-write wrapper for an input-state bundle whose DSL-owned seed facts
/// came from a generated MeerkatMachine-owned snapshot.
#[derive(Debug, Clone)]
pub struct InputStatePersistenceRecord {
    bundle: StoredInputState,
    expected_row_digest: Option<String>,
}

impl InputStatePersistenceRecord {
    /// Package a store-bound input-state bundle that was read from generated
    /// MeerkatMachine authority. This is intentionally crate-private so
    /// callers cannot mint persistence records from handwritten seed facts.
    pub(crate) fn from_machine_snapshot(bundle: StoredInputState) -> Result<Self, String> {
        crate::meerkat_machine::authorize_stored_input_state_seed(
            &bundle.state.input_id,
            &bundle.seed,
        )?;
        Ok(Self {
            bundle,
            expected_row_digest: None,
        })
    }

    /// Fence this update on the exact stored row bytes it was derived from
    /// (domain-prefixed SHA-256, as reported by
    /// `RuntimeStore::load_input_states_with_versions`). A store applying a
    /// fenced record MUST verify the current stored row still hashes to this
    /// digest inside the same transaction and fail the whole boundary with
    /// `RuntimeStoreError::InputRowVersionConflict` otherwise. Cold recovery
    /// uses this: between loading a row and committing the recovered
    /// boundary, another process may advance, adopt, or terminalize the
    /// input, and a blind upsert would overwrite the newer truth.
    pub(crate) fn with_expected_row_digest(mut self, digest: String) -> Self {
        self.expected_row_digest = Some(digest);
        self
    }

    /// Exact prior row digest this update is fenced on, when present.
    pub fn expected_row_digest(&self) -> Option<&str> {
        self.expected_row_digest.as_deref()
    }

    /// Raw bundle approved for durable persistence.
    pub fn as_stored(&self) -> &StoredInputState {
        &self.bundle
    }

    /// Clone the approved raw bundle.
    pub fn clone_stored(&self) -> StoredInputState {
        self.bundle.clone()
    }

    /// Consume the approved record into its raw bundle.
    pub fn into_stored(self) -> StoredInputState {
        self.bundle
    }

    /// Consume the approved record into its raw bundle plus the expected
    /// prior row digest it is fenced on.
    pub fn into_stored_and_expected(self) -> (StoredInputState, Option<String>) {
        (self.bundle, self.expected_row_digest)
    }
}

/// Per-input shell data. Plain fields, no hidden state machine.
///
/// All DSL-owned lifecycle fields (`phase`, `last_run_id`,
/// `last_boundary_sequence`, `terminal_outcome`, `attempt_count`,
/// `recovery_lane`) are
/// authoritative in the DSL. Live code reads them via
/// `EphemeralRuntimeDriver::input_phase` / `input_last_run_id` /
/// `input_last_boundary_sequence` / `input_terminal_outcome` /
/// `input_attempt_count` / `input_recovery_lane`. Persistence callsites
/// serialize them via [`InputStateSeed`] bundled on [`StoredInputState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectedInputKind {
    FlowStep,
    PeerMessage,
}

impl DirectedInputKind {
    pub fn input_kind(self) -> InputKind {
        match self {
            Self::FlowStep => InputKind::FlowStep,
            Self::PeerMessage => InputKind::PeerMessage,
        }
    }
}

/// Compact admission-stamped identity retained after a directed input's
/// O(payload) replay material is retired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectedRunStartedAttribution {
    kind: DirectedInputKind,
    content_digest: String,
}

impl DirectedRunStartedAttribution {
    /// Derive attribution only from a fully validated directed input. Ordinary
    /// FlowStep and PeerMessage inputs return `None`.
    pub fn from_input(input: &Input) -> Result<Option<Self>, String> {
        if crate::input::validated_directed_interaction_id(input)?.is_none() {
            return Ok(None);
        }
        let kind = match input.kind() {
            InputKind::FlowStep => DirectedInputKind::FlowStep,
            InputKind::PeerMessage => DirectedInputKind::PeerMessage,
            other => {
                return Err(format!(
                    "directed interaction custody belongs to unsupported {other:?} input kind"
                ));
            }
        };
        let content_digest = crate::input::directed_input_run_started_content_digest(input)?;
        Ok(Some(Self {
            kind,
            content_digest,
        }))
    }

    pub fn kind(&self) -> DirectedInputKind {
        self.kind
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    fn validate(&self) -> Result<(), String> {
        if self.content_digest.is_empty() {
            return Err("directed RunStarted attribution digest is empty".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct InputState {
    pub input_id: InputId,
    pub history: Vec<InputStateHistoryEntry>,
    pub updated_at: DateTime<Utc>,
    pub policy: Option<PolicySnapshot>,
    /// Runtime-stamped run semantics captured at admission and persisted so
    /// recovery does not reclassify execution kind from payload shape.
    pub runtime_semantics: Option<RuntimeInputSemantics>,
    /// Typed input family plus digest of the exact canonical `RunStarted`
    /// content for a directed FlowStep/Peer input. This compact witness
    /// survives terminal payload retirement and binds host-journal
    /// reconstruction without retaining the original O(payload) input.
    pub directed_run_started_attribution: Option<DirectedRunStartedAttribution>,
    pub durability: Option<crate::input::InputDurability>,
    pub idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    pub recovery_count: u32,
    pub reconstruction_source: Option<ReconstructionSource>,
    /// Durable pre-finalization candidate or exact finalized public completion
    /// for this input's terminal batch.
    pub(crate) terminal_completion: Option<InputTerminalCompletion>,
    /// One-time v4 -> v5 migration witness: this input was already terminal in
    /// 0.8.10, which did not retain enough evidence to reconstruct its exact
    /// public completion. Current-version rows may carry this marker only when
    /// terminal and receipt-less.
    pub(crate) terminal_completion_unavailable: bool,
    /// Exact directed-terminal retry carrier, when this input came from the
    /// tracked cross-host flow lane.
    pub(crate) interaction_terminal_outbox: Option<InteractionTerminalOutbox>,
    /// Original ingress material retained only while crash redelivery,
    /// durable-tail attribution, or directed-terminal materialization may
    /// still need it. Authoritative terminal commits retire this payload
    /// after completion/publication obligations close; terminal history is
    /// carried by the seed and receipts above.
    pub persisted_input: Option<Input>,
    pub created_at: DateTime<Utc>,
}

impl InputState {
    /// Create a fresh InputState. Paired DSL state starts in the `Accepted`
    /// phase via [`InputStateSeed::new_accepted`]; callers that need the
    /// bundle use [`StoredInputState::new_accepted`].
    pub fn new_accepted(input_id: InputId) -> Self {
        let now = Utc::now();
        Self {
            input_id,
            history: Vec::new(),
            updated_at: now,
            policy: None,
            runtime_semantics: None,
            directed_run_started_attribution: None,
            durability: None,
            idempotency_key: None,
            recovery_count: 0,
            reconstruction_source: None,
            terminal_completion: None,
            terminal_completion_unavailable: false,
            interaction_terminal_outbox: None,
            persisted_input: None,
            created_at: now,
        }
    }

    pub fn history(&self) -> &[InputStateHistoryEntry] {
        &self.history
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// Custom Serialize / Deserialize — preserves the on-disk wire format
// ---------------------------------------------------------------------------
//
// `InputStateSerde` is the on-disk contract exercised by
// `recovery_contract`, `recovery_replay`, and `driver_persistent` tests.
// Legacy field names, types, defaults, and `skip_serializing_if` markers stay
// stable. The v5 compact directed attribution is additive and defaults absent
// while v3/v4 rows derive it from their still-retained replay payload.
// Serialization flows through [`StoredInputState`] so shell + generated seed
// facts remain one store-bound wire row.

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Serialize, Deserialize)]
struct InputStateSerde {
    stored_input_state_version: u32,
    input_id: InputId,
    current_state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<PolicySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_semantics: Option<RuntimeInputSemantics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directed_run_started_attribution: Option<DirectedRunStartedAttribution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_outcome: Option<InputTerminalOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durability: Option<crate::input::InputDurability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    #[serde(default)]
    attempt_count: u32,
    #[serde(default)]
    recovery_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<InputStateHistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reconstruction_source: Option<ReconstructionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_completion: Option<InputTerminalCompletion>,
    #[serde(default, skip_serializing_if = "is_false")]
    terminal_completion_unavailable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interaction_terminal_outbox: Option<InteractionTerminalOutbox>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    persisted_input: Option<Input>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_boundary_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_lane: Option<HandlingMode>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Serialize for StoredInputState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.state.terminal_completion_unavailable
            && (self.seed.terminal_outcome.is_none() || self.state.terminal_completion.is_some())
        {
            return Err(serde::ser::Error::custom(
                "terminal completion unavailable marker has an invalid v5 shape",
            ));
        }
        let helper = InputStateSerde {
            stored_input_state_version:
                meerkat_core::generated::session_persistence_version_authority::stored_input_state_version(
                ),
            input_id: self.state.input_id.clone(),
            current_state: self.seed.phase,
            policy: self.state.policy.clone(),
            runtime_semantics: self.state.runtime_semantics,
            directed_run_started_attribution: self
                .state
                .directed_run_started_attribution
                .clone(),
            terminal_outcome: self.seed.terminal_outcome.clone(),
            durability: self.state.durability,
            idempotency_key: self.state.idempotency_key.clone(),
            attempt_count: self.seed.attempt_count,
            recovery_count: self.state.recovery_count,
            history: self.state.history.clone(),
            reconstruction_source: self.state.reconstruction_source.clone(),
            terminal_completion: self.state.terminal_completion.clone(),
            terminal_completion_unavailable: self.state.terminal_completion_unavailable,
            interaction_terminal_outbox: self.state.interaction_terminal_outbox.clone(),
            persisted_input: self.state.persisted_input.clone(),
            last_run_id: self.seed.last_run_id.clone(),
            last_boundary_sequence: self.seed.last_boundary_sequence,
            admission_sequence: self.seed.admission_sequence,
            recovery_lane: self.seed.recovery_lane,
            created_at: self.state.created_at,
            updated_at: self.state.updated_at,
        };
        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredInputState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let helper = InputStateSerde::deserialize(deserializer)?;
        let observed_stored_input_state_version = helper.stored_input_state_version;
        let _stored_input_state_version =
            meerkat_core::generated::session_persistence_version_authority::restore_stored_input_state_version(
                observed_stored_input_state_version,
            )
            .map_err(<D::Error as serde::de::Error>::custom)?;
        if observed_stored_input_state_version < 5 && helper.terminal_completion.is_some() {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored input state before v5 cannot carry a terminal completion receipt",
            ));
        }
        if observed_stored_input_state_version < 5 && helper.terminal_completion_unavailable {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored input state before v5 cannot carry a completion-unavailable marker",
            ));
        }
        if observed_stored_input_state_version < 5
            && helper.directed_run_started_attribution.is_some()
        {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored input state before v5 cannot carry compact directed attribution",
            ));
        }
        if observed_stored_input_state_version == 3 && helper.interaction_terminal_outbox.is_some()
        {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored input state v3 cannot carry an interaction terminal outbox",
            ));
        }
        if let Some(outbox) = helper.interaction_terminal_outbox.as_ref() {
            outbox
                .validate()
                .map_err(<D::Error as serde::de::Error>::custom)?;
        }
        if let Some(completion) = helper.terminal_completion.as_ref() {
            if completion.input_id != helper.input_id || helper.terminal_outcome.is_none() {
                return Err(<D::Error as serde::de::Error>::custom(
                    "terminal completion row is not bound to the same terminal input state",
                ));
            }
            completion
                .validate_row()
                .map_err(<D::Error as serde::de::Error>::custom)?;
        }
        let terminal_completion_unavailable = if observed_stored_input_state_version < 5 {
            helper.terminal_outcome.is_some() && helper.terminal_completion.is_none()
        } else {
            helper.terminal_completion_unavailable
        };
        if terminal_completion_unavailable
            && (helper.terminal_outcome.is_none() || helper.terminal_completion.is_some())
        {
            return Err(<D::Error as serde::de::Error>::custom(
                "terminal completion unavailable marker has an invalid v5 shape",
            ));
        }
        if let Some(stored) = helper.directed_run_started_attribution.as_ref() {
            stored
                .validate()
                .map_err(<D::Error as serde::de::Error>::custom)?;
        }
        let payload_directed_attribution = helper
            .persisted_input
            .as_ref()
            .map(DirectedRunStartedAttribution::from_input)
            .transpose()
            .map_err(<D::Error as serde::de::Error>::custom)?
            .flatten();
        if helper.directed_run_started_attribution.is_some()
            && helper.persisted_input.is_some()
            && payload_directed_attribution.is_none()
        {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored directed RunStarted attribution belongs to a non-directed replay payload",
            ));
        }
        if helper.directed_run_started_attribution.is_some()
            && payload_directed_attribution.is_some()
            && helper.directed_run_started_attribution != payload_directed_attribution
        {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored directed RunStarted attribution disagrees with the retained replay payload",
            ));
        }
        let directed_run_started_attribution = helper
            .directed_run_started_attribution
            .or(payload_directed_attribution);
        let state = InputState {
            input_id: helper.input_id,
            history: helper.history,
            updated_at: helper.updated_at,
            policy: helper.policy,
            runtime_semantics: helper.runtime_semantics,
            directed_run_started_attribution,
            durability: helper.durability,
            idempotency_key: helper.idempotency_key,
            recovery_count: helper.recovery_count,
            reconstruction_source: helper.reconstruction_source,
            terminal_completion: helper.terminal_completion,
            terminal_completion_unavailable,
            interaction_terminal_outbox: helper.interaction_terminal_outbox,
            persisted_input: helper.persisted_input,
            created_at: helper.created_at,
        };
        let seed = InputStateSeed {
            phase: helper.current_state,
            last_run_id: helper.last_run_id,
            last_boundary_sequence: helper.last_boundary_sequence,
            admission_sequence: helper.admission_sequence,
            terminal_outcome: helper.terminal_outcome,
            attempt_count: helper.attempt_count,
            recovery_lane: helper.recovery_lane,
        };
        Ok(StoredInputState { state, seed })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };
    use meerkat_core::ops::{OpEvent, OperationId};

    fn terminal_outbox_batch_fixture() -> Vec<InteractionTerminalOutbox> {
        let mut completion_input_ids = vec![InputId::new(), InputId::new(), InputId::new()];
        completion_input_ids.sort_by_key(|input_id| input_id.0);
        let directed_input_ids = vec![
            completion_input_ids[0].clone(),
            completion_input_ids[2].clone(),
        ];
        let candidate = InteractionTerminalCandidate::CompletedWithoutResult;
        let candidate_digest = interaction_terminal_payload_digest(&candidate).unwrap();
        let completion_input_ids_digest =
            interaction_terminal_payload_digest(&completion_input_ids).unwrap();
        let candidate_owner_input_id = directed_input_ids[0].clone();
        let batch_key = InteractionTerminalBatchKey::Run {
            run_id: RunId::new(),
        };
        directed_input_ids
            .into_iter()
            .enumerate()
            .map(|(ordinal, input_id)| {
                let owns_candidate = input_id == candidate_owner_input_id;
                InteractionTerminalOutbox {
                    interaction_id: InteractionId(input_id.0),
                    input_id,
                    batch_ordinal: ordinal as u16,
                    batch_key: batch_key.clone(),
                    owner_session_id: SessionId::new(),
                    owner_agent_runtime_id: Some("fixture-runtime".to_string()),
                    owner_fence_token: Some(7),
                    owner_runtime_generation: Some(3),
                    owner_runtime_epoch_id: Some("fixture-epoch".to_string()),
                    candidate_owner_input_id: candidate_owner_input_id.clone(),
                    candidate: owns_candidate.then(|| candidate.clone()),
                    candidate_digest: candidate_digest.clone(),
                    completion_input_ids: owns_candidate.then(|| completion_input_ids.clone()),
                    completion_input_ids_digest: completion_input_ids_digest.clone(),
                    phase: InteractionTerminalOutboxPhase::Candidate,
                }
            })
            .collect()
    }

    fn pending_terminal_completion_batch_fixture() -> Vec<StoredInputState> {
        let mut input_ids = vec![InputId::new(), InputId::new()];
        input_ids.sort_by_key(|input_id| input_id.0);
        let owner_input_id = input_ids[0].clone();
        let candidate = InteractionTerminalCandidate::MachineTerminalFailure {
            error: meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                "executor failed after applying the input boundary",
            ),
        };
        let candidate_digest = interaction_terminal_payload_digest(&candidate).unwrap();
        let completion_input_ids_digest = interaction_terminal_payload_digest(&input_ids).unwrap();
        let batch_key = InputTerminalCompletionBatchKey::Run {
            run_id: RunId::new(),
        };
        input_ids
            .iter()
            .enumerate()
            .map(|(ordinal, input_id)| {
                let owns_payload = input_id == &owner_input_id;
                let mut stored = StoredInputState::new_accepted(input_id.clone());
                stored.seed.phase = InputLifecycleState::Consumed;
                stored.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                stored.state.terminal_completion = Some(InputTerminalCompletion {
                    input_id: input_id.clone(),
                    batch_ordinal: ordinal as u16,
                    batch_key: batch_key.clone(),
                    owner_input_id: owner_input_id.clone(),
                    candidate_digest: candidate_digest.clone(),
                    completion_input_ids_digest: completion_input_ids_digest.clone(),
                    requires_session_checkpoint: true,
                    candidate: owns_payload.then(|| candidate.clone()),
                    completion_input_ids: owns_payload.then(|| input_ids.clone()),
                    outcome: None,
                    phase: InputTerminalCompletionPhase::Pending,
                });
                stored
            })
            .collect()
    }

    fn restart_terminal_completion_rows(rows: Vec<StoredInputState>) -> Vec<StoredInputState> {
        rows.into_iter()
            .map(|row| {
                let bytes = serde_json::to_vec(&row).unwrap();
                serde_json::from_slice(&bytes).unwrap()
            })
            .collect()
    }

    #[test]
    fn restart_after_terminal_transaction_observes_pending_exact_receipt() {
        let rows = restart_terminal_completion_rows(pending_terminal_completion_batch_fixture());
        let input_id = rows[1].state.input_id.clone();
        let carriers = rows
            .iter()
            .map(|row| row.state.terminal_completion.clone().unwrap())
            .collect::<Vec<_>>();

        validate_input_terminal_completion_batch(&carriers).unwrap();
        assert!(
            input_terminal_completion_outcome(&rows, &input_id)
                .unwrap()
                .is_none(),
            "a kill after the terminal transaction must recover the candidate, not invent a final outcome"
        );
    }

    #[test]
    fn restart_after_receipt_cas_recovers_exact_public_outcome() {
        let mut rows = pending_terminal_completion_batch_fixture();
        let input_id = rows[1].state.input_id.clone();
        let outcome = crate::completion::CompletionOutcome::CompletedWithFinalizationFailure {
            error: meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                "checkpoint rejected the committed snapshot",
            ),
        };
        let finalization = InputTerminalCompletionFinalizationVerdict::Failed;
        let receipt_digest =
            interaction_terminal_payload_digest(&(&outcome, finalization)).unwrap();
        for row in &mut rows {
            let completion = row.state.terminal_completion.as_mut().unwrap();
            completion.candidate = None;
            completion.outcome =
                (completion.input_id == completion.owner_input_id).then(|| outcome.clone());
            completion.phase = InputTerminalCompletionPhase::Finalized {
                receipt_digest: receipt_digest.clone(),
                finalization,
            };
        }

        let mut forged_verdict = serde_json::to_value(&rows[0]).unwrap();
        forged_verdict["terminal_completion"]["phase"]["finalization"] =
            serde_json::json!("succeeded");
        let error = serde_json::from_value::<StoredInputState>(forged_verdict)
            .expect_err("the receipt digest must bind the typed finalization verdict");
        assert!(error.to_string().contains("receipt digest mismatch"));

        let rows = restart_terminal_completion_rows(rows);
        let recovered = input_terminal_completion_outcome(&rows, &input_id)
            .unwrap()
            .expect("a kill after receipt CAS must recover the exact outcome");
        assert_eq!(
            serde_json::to_value(recovered).unwrap(),
            serde_json::to_value(outcome).unwrap()
        );
    }

    #[test]
    fn terminal_outbox_batch_preserves_full_mixed_completion_recipients() {
        let outboxes = terminal_outbox_batch_fixture();
        let recipients = validate_unpublished_interaction_terminal_outbox_batch(&outboxes).unwrap();

        assert_eq!(recipients.len(), 3);
        assert_eq!(outboxes.len(), 2);
        assert!(recipients.contains(&outboxes[0].input_id));
        assert!(recipients.contains(&outboxes[1].input_id));
    }

    #[test]
    fn terminal_outbox_batch_rejects_noncontiguous_or_reordered_ordinals() {
        let mut outboxes = terminal_outbox_batch_fixture();
        outboxes[1].batch_ordinal = 2;
        assert!(
            validate_unpublished_interaction_terminal_outbox_batch(&outboxes)
                .unwrap_err()
                .contains("ordinals are not contiguous")
        );
    }

    #[test]
    fn terminal_outbox_owner_rejects_duplicate_completion_recipients() {
        let mut outboxes = terminal_outbox_batch_fixture();
        let owner = &mut outboxes[0];
        let recipients = owner.completion_input_ids.as_mut().unwrap();
        recipients.push(recipients[0].clone());
        owner.completion_input_ids_digest =
            interaction_terminal_payload_digest(recipients).unwrap();

        assert!(
            owner
                .validate()
                .unwrap_err()
                .contains("contains duplicates")
        );
    }

    #[test]
    fn terminal_outbox_resource_bounds_reject_257_rows_or_recipients() {
        let fixture = terminal_outbox_batch_fixture();
        let oversized_rows = vec![fixture[0].clone(); 257];
        assert!(
            validate_interaction_terminal_outbox_batch_shape(&oversized_rows)
                .unwrap_err()
                .contains("invalid directed-row count")
        );

        let mut owner = fixture[0].clone();
        let recipients = (0..257).map(|_| InputId::new()).collect::<Vec<_>>();
        owner.completion_input_ids_digest =
            interaction_terminal_payload_digest(&recipients).unwrap();
        owner.completion_input_ids = Some(recipients);
        assert!(
            owner
                .validate()
                .unwrap_err()
                .contains("recipient set has invalid size")
        );
    }

    #[test]
    fn published_terminal_outbox_compaction_retains_only_immutable_proofs() {
        let mut outbox = terminal_outbox_batch_fixture().remove(0);
        let recipient_digest = outbox.completion_input_ids_digest.clone();
        let candidate_digest = outbox.candidate_digest.clone();
        outbox.candidate = None;
        outbox.completion_input_ids = None;
        outbox.phase = InteractionTerminalOutboxPhase::Published {
            finalization_failed: false,
            publication: InteractionTerminalPublication {
                terminal_seq: 9,
                payload_digest: "published-event-digest".to_string(),
            },
        };

        outbox.validate().unwrap();
        assert_eq!(outbox.candidate_digest, candidate_digest);
        assert_eq!(outbox.completion_input_ids_digest, recipient_digest);
    }

    #[test]
    fn published_terminal_batch_rejects_split_immutable_recipient_proof() {
        let mut outboxes = terminal_outbox_batch_fixture();
        for outbox in &mut outboxes {
            outbox.candidate = None;
            outbox.completion_input_ids = None;
            outbox.phase = InteractionTerminalOutboxPhase::Published {
                finalization_failed: false,
                publication: InteractionTerminalPublication {
                    terminal_seq: u64::from(outbox.batch_ordinal) + 1,
                    payload_digest: format!("event-{}", outbox.batch_ordinal),
                },
            };
        }
        outboxes[1].completion_input_ids_digest = "split-proof".to_string();

        assert!(
            validate_interaction_terminal_outbox_batch_shape(&outboxes)
                .unwrap_err()
                .contains("split immutable identity")
        );
    }

    #[test]
    fn new_accepted_starts_with_no_shell_history() {
        let id = InputId::new();
        let state = InputState::new_accepted(id.clone());
        assert_eq!(state.input_id, id);
        assert!(state.history.is_empty());
    }

    #[test]
    fn seed_new_accepted_defaults_match_queue_lifecycle() {
        let seed = InputStateSeed::new_accepted();
        assert_eq!(seed.phase, InputLifecycleState::Accepted);
        assert!(seed.last_run_id.is_none());
        assert!(seed.last_boundary_sequence.is_none());
        assert!(seed.admission_sequence.is_none());
        assert!(seed.terminal_outcome.is_none());
        assert_eq!(seed.attempt_count, 0);
    }

    #[test]
    fn lifecycle_state_serde() {
        for state in [
            InputLifecycleState::Accepted,
            InputLifecycleState::Queued,
            InputLifecycleState::Staged,
            InputLifecycleState::Applied,
            InputLifecycleState::AppliedPendingConsumption,
            InputLifecycleState::Consumed,
            InputLifecycleState::Superseded,
            InputLifecycleState::Coalesced,
            InputLifecycleState::Abandoned,
        ] {
            let json = serde_json::to_value(state).unwrap();
            let parsed: InputLifecycleState = serde_json::from_value(json).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn stored_input_state_serde_roundtrip_preserves_fields() {
        let mut state = InputState::new_accepted(InputId::new());
        let policy = PolicyDecision {
            apply_mode: ApplyMode::StageRunStart,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: PolicyVersion(1),
        };
        state.policy = Some(PolicySnapshot {
            version: PolicyVersion(1),
            decision: policy.clone(),
        });
        state.runtime_semantics = Some(
            crate::policy_table::generated_admission_projection_for_kind(
                crate::identifiers::KindId::new(crate::identifiers::InputKind::Prompt),
                true,
            )
            .expect("generated admission projection")
            .runtime_semantics,
        );
        state.history.push(InputStateHistoryEntry {
            timestamp: state.updated_at,
            from: InputLifecycleState::Accepted,
            to: InputLifecycleState::Queued,
            reason: Some("QueueAccepted".into()),
        });
        let bundle = StoredInputState {
            state,
            seed: InputStateSeed {
                phase: InputLifecycleState::Queued,
                last_run_id: None,
                last_boundary_sequence: None,
                admission_sequence: Some(42),
                terminal_outcome: None,
                attempt_count: 0,
                recovery_lane: Some(HandlingMode::Queue),
            },
        };

        let json = serde_json::to_value(&bundle).unwrap();
        let parsed: StoredInputState = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.state.input_id, bundle.state.input_id);
        assert_eq!(parsed.seed.phase, bundle.seed.phase);
        assert_eq!(
            parsed.seed.admission_sequence,
            bundle.seed.admission_sequence
        );
        assert_eq!(parsed.seed.recovery_lane, bundle.seed.recovery_lane);
        assert_eq!(
            parsed.state.runtime_semantics,
            bundle.state.runtime_semantics
        );
        assert_eq!(parsed.state.history.len(), 1);
    }

    #[test]
    fn v4_directed_attribution_migrates_from_payload_but_ordinary_flow_stays_untracked() {
        let stable = uuid::Uuid::from_u128(0x00000000000040008000000000000123);
        let directed = crate::mob_adapter::create_tracked_flow_step_input(
            "step-1",
            meerkat_core::types::ContentInput::Text("directed".to_string()),
            "flow-1",
            None,
            &stable.to_string(),
        )
        .expect("directed fixture");
        let mut directed_row = StoredInputState::new_accepted(directed.id().clone());
        directed_row.state.persisted_input = Some(directed);
        let mut directed_json = serde_json::to_value(directed_row).expect("serialize fixture");
        directed_json["stored_input_state_version"] = serde_json::json!(4);
        directed_json
            .as_object_mut()
            .expect("fixture is an object")
            .remove("directed_run_started_attribution");
        let migrated: StoredInputState =
            serde_json::from_value(directed_json).expect("v4 directed row migrates from payload");
        assert!(migrated.state.directed_run_started_attribution.is_some());

        let ordinary = crate::mob_adapter::create_flow_step_input(
            "step-2",
            meerkat_core::types::ContentInput::Text("ordinary".to_string()),
            "flow-1",
            2,
            None,
        );
        let mut ordinary_row = StoredInputState::new_accepted(ordinary.id().clone());
        ordinary_row.state.persisted_input = Some(ordinary);
        let mut ordinary_json = serde_json::to_value(ordinary_row).expect("serialize fixture");
        ordinary_json["stored_input_state_version"] = serde_json::json!(4);
        let restored: StoredInputState =
            serde_json::from_value(ordinary_json).expect("ordinary v4 flow row remains valid");
        assert!(restored.state.directed_run_started_attribution.is_none());
    }

    #[test]
    fn compact_directed_attribution_must_match_retained_payload() {
        let stable = uuid::Uuid::from_u128(0x00000000000040008000000000000456);
        let input = crate::mob_adapter::create_tracked_flow_step_input(
            "step-1",
            meerkat_core::types::ContentInput::Text("directed".to_string()),
            "flow-1",
            None,
            &stable.to_string(),
        )
        .expect("directed fixture");
        let mut row = StoredInputState::new_accepted(input.id().clone());
        row.state.directed_run_started_attribution =
            DirectedRunStartedAttribution::from_input(&input)
                .expect("valid attribution derivation");
        row.state.persisted_input = Some(input);
        let mut json = serde_json::to_value(row).expect("serialize fixture");
        json["directed_run_started_attribution"]["content_digest"] =
            serde_json::json!("wrong-digest");

        let error = serde_json::from_value::<StoredInputState>(json)
            .expect_err("mismatched compact attribution must fail closed");
        assert!(
            error
                .to_string()
                .contains("disagrees with the retained replay payload")
        );
    }

    /// v0.8.7 regression witness (release-bricking class): a stored-input-state
    /// v4 row whose interaction terminal outbox carries the
    /// pre-durable-callback `callback_pending` candidate shape (no
    /// `tool_use_id`) must decode AND keep verifying against its stored
    /// candidate digest — v0.8.7 computed that digest over exactly these
    /// bytes, so the decoded candidate must re-serialize byte-identically.
    #[test]
    fn stored_input_state_v087_callback_pending_row_still_decodes() {
        let candidate_json =
            r#"{"candidate_type":"callback_pending","tool_name":"external","args":{"value":1}}"#;
        let completion_ids_json = r#"["00000000-0000-0000-0000-0000000000aa"]"#;
        let candidate_digest = format!("{:x}", Sha256::digest(candidate_json.as_bytes()));
        let completion_ids_digest = format!("{:x}", Sha256::digest(completion_ids_json.as_bytes()));
        let row = format!(
            r#"{{
                "stored_input_state_version": 4,
                "input_id": "00000000-0000-0000-0000-0000000000aa",
                "current_state": "applied",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "interaction_terminal_outbox": {{
                    "interaction_id": "00000000-0000-0000-0000-0000000000aa",
                    "input_id": "00000000-0000-0000-0000-0000000000aa",
                    "batch_ordinal": 0,
                    "batch_key": {{"scope":"run","run_id":"00000000-0000-0000-0000-0000000000bb"}},
                    "owner_session_id": "00000000-0000-0000-0000-0000000000cc",
                    "owner_agent_runtime_id": "runtime-a",
                    "owner_fence_token": 7,
                    "owner_runtime_generation": 3,
                    "owner_runtime_epoch_id": "epoch-3",
                    "candidate_owner_input_id": "00000000-0000-0000-0000-0000000000aa",
                    "candidate": {candidate_json},
                    "candidate_digest": "{candidate_digest}",
                    "completion_input_ids": {completion_ids_json},
                    "completion_input_ids_digest": "{completion_ids_digest}",
                    "phase": {{"phase":"candidate"}}
                }}
            }}"#
        );

        let restored: StoredInputState =
            serde_json::from_str(&row).expect("v0.8.7 callback-pending row must decode");
        let outbox = restored
            .state
            .interaction_terminal_outbox
            .as_ref()
            .expect("outbox survives decode");
        let candidate = outbox.candidate.as_ref().expect("owner keeps candidate");
        assert!(matches!(
            candidate,
            InteractionTerminalCandidate::CallbackPending {
                tool_use_id: None,
                ..
            }
        ));
        assert_eq!(
            interaction_terminal_payload_digest(candidate).unwrap(),
            outbox.candidate_digest,
            "legacy candidate must re-serialize byte-identically under its stored digest"
        );
        // Recovery projects the unknown identity as empty, never a fabricated id.
        assert!(matches!(
            candidate.core_apply_terminal(),
            Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::CallbackPending {
                tool_use_id,
                ..
            }) if tool_use_id.is_empty()
        ));
    }

    /// The legacy (identity-less) callback candidate pairs with both event
    /// shapes it can durably meet: a v0.8.7-finalized event (no pending set)
    /// and an event this binary finalizes from the same legacy candidate.
    /// A candidate WITH identity still demands the exact pending set.
    #[test]
    fn legacy_callback_candidate_matches_legacy_and_reprojected_events() {
        let interaction_id = InteractionId(uuid::Uuid::new_v4());
        let args = serde_json::json!({"value": 1});
        let legacy_candidate = InteractionTerminalCandidate::CallbackPending {
            tool_use_id: None,
            tool_name: "external".to_string(),
            args: args.clone(),
        };
        let event = |pending_tool_calls| AgentEvent::InteractionCallbackPending {
            interaction_id,
            tool_name: "external".to_string(),
            args: args.clone(),
            pending_tool_calls,
        };

        let legacy_event = event(Vec::new());
        let reprojected_event = event(vec![meerkat_core::error::PendingCallbackToolCall {
            tool_use_id: String::new(),
            tool_name: "external".to_string(),
            args: args.clone(),
        }]);
        assert!(interaction_terminal_candidate_matches_event(
            &legacy_candidate,
            interaction_id,
            &legacy_event,
            false,
        ));
        assert!(interaction_terminal_candidate_matches_event(
            &legacy_candidate,
            interaction_id,
            &reprojected_event,
            false,
        ));

        let modern_candidate = InteractionTerminalCandidate::CallbackPending {
            tool_use_id: Some("call-9".to_string()),
            tool_name: "external".to_string(),
            args: args.clone(),
        };
        assert!(!interaction_terminal_candidate_matches_event(
            &modern_candidate,
            interaction_id,
            &legacy_event,
            false,
        ));
        let exact_event = event(vec![meerkat_core::error::PendingCallbackToolCall {
            tool_use_id: "call-9".to_string(),
            tool_name: "external".to_string(),
            args: args.clone(),
        }]);
        assert!(interaction_terminal_candidate_matches_event(
            &modern_candidate,
            interaction_id,
            &exact_event,
            false,
        ));
    }

    #[test]
    fn stored_input_state_unknown_versions_still_fail_closed() {
        let mut fixture =
            serde_json::to_value(StoredInputState::new_accepted(InputId::new())).unwrap();
        for rejected in [2, 6] {
            fixture["stored_input_state_version"] = serde_json::json!(rejected);
            let error = serde_json::from_value::<StoredInputState>(fixture.clone())
                .expect_err("unknown historical and future versions must fail closed");
            assert!(error.to_string().contains("expected current 5"));
        }

        // 0.8.10 accepted still-retained v3 input rows lazily, so a supported
        // 0.8.10 deployment may legitimately present that exact row version.
        fixture["stored_input_state_version"] = serde_json::json!(3);
        serde_json::from_value::<StoredInputState>(fixture)
            .expect("released v3 row retained by 0.8.10 remains supported");
    }

    #[test]
    fn stored_input_state_v4_migrates_to_v5_without_inventing_a_completion_receipt() {
        let mut fixture =
            serde_json::to_value(StoredInputState::new_accepted(InputId::new())).unwrap();
        fixture["stored_input_state_version"] = serde_json::json!(4);
        fixture
            .as_object_mut()
            .expect("stored input state fixture is an object")
            .remove("terminal_completion");

        let restored: StoredInputState =
            serde_json::from_value(fixture).expect("0.8.10 v4 input state must migrate");
        assert!(restored.state.terminal_completion.is_none());

        let migrated = serde_json::to_value(restored).unwrap();
        assert_eq!(
            migrated["stored_input_state_version"],
            meerkat_core::generated::session_persistence_version_authority::STORED_INPUT_STATE_VERSION,
        );

        let mut terminal_fixture =
            serde_json::to_value(StoredInputState::new_accepted(InputId::new())).unwrap();
        terminal_fixture["stored_input_state_version"] = serde_json::json!(4);
        terminal_fixture["current_state"] = serde_json::json!("consumed");
        terminal_fixture["terminal_outcome"] = serde_json::json!({ "outcome_type": "consumed" });
        let restored_terminal: StoredInputState = serde_json::from_value(terminal_fixture)
            .expect("0.8.10 terminal row must retain an explicit evidence-gap marker");
        assert!(restored_terminal.state.terminal_completion_unavailable);
        assert!(matches!(
            input_terminal_completion_outcome(
                std::slice::from_ref(&restored_terminal),
                &restored_terminal.state.input_id,
            ),
            Err(InputTerminalCompletionReadError::MigratedReceiptUnavailable)
        ));
        let migrated_terminal = serde_json::to_value(restored_terminal).unwrap();
        assert_eq!(
            migrated_terminal["terminal_completion_unavailable"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn stored_input_state_rejects_legacy_persisted_input_tags() {
        // Pre-rename `system_generated` / `projected` persisted input tags are
        // retired shapes: a stored row carrying them must fail closed instead
        // of being folded into the canonical `continuation` / `operation` tags.
        let continuation_bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Continuation(
                    crate::input::ContinuationInput::detached_background_op_completed(),
                )),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut continuation_json = serde_json::to_value(&continuation_bundle).unwrap();
        continuation_json["persisted_input"]["input_type"] =
            serde_json::Value::String("system_generated".into());
        serde_json::from_value::<StoredInputState>(continuation_json)
            .expect_err("legacy system_generated persisted input tag must be rejected");

        let operation_bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Operation(crate::input::OperationInput {
                    header: crate::input::InputHeader {
                        id: InputId::new(),
                        timestamp: Utc::now(),
                        source: crate::input::InputOrigin::System,
                        durability: crate::input::InputDurability::Derived,
                        visibility: crate::input::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    operation_id: OperationId::new(),
                    event: OpEvent::Cancelled {
                        id: OperationId::new(),
                    },
                })),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut operation_json = serde_json::to_value(&operation_bundle).unwrap();
        operation_json["persisted_input"]["input_type"] =
            serde_json::Value::String("projected".into());
        serde_json::from_value::<StoredInputState>(operation_json)
            .expect_err("legacy projected persisted input tag must be rejected");
    }

    #[test]
    fn stored_input_state_rejects_legacy_dual_carrier_persisted_input_shape() {
        // The retired persisted prompt shape carried `text` + optional
        // `blocks`; the single typed `content` owner replaced both. A stored
        // row holding the old shape must fail closed.
        let bundle = StoredInputState {
            state: InputState {
                persisted_input: Some(Input::Prompt(crate::input::PromptInput::new("hello", None))),
                ..InputState::new_accepted(InputId::new())
            },
            seed: InputStateSeed::new_accepted(),
        };
        let mut json = serde_json::to_value(&bundle).unwrap();
        let persisted = json["persisted_input"]
            .as_object_mut()
            .expect("persisted_input object");
        persisted.remove("content");
        persisted.insert("text".into(), serde_json::Value::String("hello".into()));
        persisted.insert("blocks".into(), serde_json::Value::Null);
        serde_json::from_value::<StoredInputState>(json)
            .expect_err("legacy text+blocks persisted prompt shape must be rejected");
    }

    #[test]
    fn abandon_reason_serde() {
        for reason in [
            InputAbandonReason::Retired,
            InputAbandonReason::Reset,
            InputAbandonReason::Destroyed,
            InputAbandonReason::Cancelled,
        ] {
            let json = serde_json::to_value(&reason).unwrap();
            let parsed: InputAbandonReason = serde_json::from_value(json).unwrap();
            assert_eq!(reason, parsed);
        }
    }

    #[test]
    fn terminal_outcome_consumed_serde() {
        let outcome = InputTerminalOutcome::Consumed;
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "consumed");
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert_eq!(outcome, parsed);
    }

    #[test]
    fn terminal_outcome_superseded_serde() {
        let outcome = InputTerminalOutcome::Superseded {
            superseded_by: InputId::new(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "superseded");
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, InputTerminalOutcome::Superseded { .. }));
    }

    #[test]
    fn terminal_outcome_abandoned_serde() {
        let outcome = InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Retired,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Retired,
            }
        ));
    }

    #[test]
    fn callback_batch_candidate_accepts_abandoned_projection_on_finalization_failure() {
        let interaction_id = InteractionId(uuid::Uuid::new_v4());
        let candidate = InteractionTerminalCandidate::CallbackBatchPending {
            pending_tool_calls: vec![meerkat_core::error::PendingCallbackToolCall {
                tool_use_id: "call-1".to_string(),
                tool_name: "external".to_string(),
                args: serde_json::json!({"value": 1}),
            }],
        };
        let event = AgentEvent::InteractionFailed {
            interaction_id,
            reason: meerkat_core::event::InteractionFailureReason::abandoned(
                "terminal publication failed",
            ),
        };

        assert!(interaction_terminal_candidate_matches_event(
            &candidate,
            interaction_id,
            &event,
            true,
        ));
    }

    #[test]
    fn reconstruction_source_serde() {
        let sources = vec![
            ReconstructionSource::Projection {
                rule_id: "rule-1".into(),
                source_event_id: "evt-1".into(),
            },
            ReconstructionSource::Coalescing {
                source_input_ids: vec![InputId::new(), InputId::new()],
            },
        ];
        for source in sources {
            let json = serde_json::to_value(&source).unwrap();
            assert!(json["source_type"].is_string());
            let parsed: ReconstructionSource = serde_json::from_value(json).unwrap();
            let _ = parsed;
        }
    }

    #[test]
    fn input_state_event_serde() {
        let event = InputStateEvent {
            timestamp: Utc::now(),
            state: InputLifecycleState::Queued,
            detail: Some("queued for processing".into()),
        };
        let json = serde_json::to_value(&event).unwrap();
        let parsed: InputStateEvent = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.state, InputLifecycleState::Queued);
    }
}
