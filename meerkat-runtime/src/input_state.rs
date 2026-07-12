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

use crate::identifiers::PolicyVersion;
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
        tool_name: String,
        args: serde_json::Value,
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
    pub(crate) fn core_apply_terminal(
        &self,
    ) -> Option<meerkat_core::lifecycle::core_executor::CoreApplyTerminal> {
        use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
        match self {
            Self::RunResult { result } => Some(CoreApplyTerminal::RunResult(result.clone())),
            Self::CompletedWithoutResult => Some(CoreApplyTerminal::NoPendingBoundary),
            Self::CallbackPending { tool_name, args } => Some(CoreApplyTerminal::CallbackPending {
                tool_name: tool_name.clone(),
                args: args.clone(),
            }),
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
            Self::CallbackPending { .. } => RuntimeCompletionTerminalObservation::CallbackPending,
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
            tool_name, args, ..
        } => Some(AgentEvent::InteractionCallbackPending {
            interaction_id,
            tool_name: tool_name.clone(),
            args: args.clone(),
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
            InteractionTerminalCandidate::CallbackPending { tool_name, args },
            AgentEvent::InteractionCallbackPending {
                tool_name: event_tool,
                args: event_args,
                ..
            },
        ) => tool_name == event_tool && args == event_args,
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

impl StoredInputState {
    /// Convenience: freshly-accepted bundle.
    pub fn new_accepted(input_id: InputId) -> Self {
        Self {
            state: InputState::new_accepted(input_id),
            seed: InputStateSeed::new_accepted(),
        }
    }
}

/// Store-write wrapper for an input-state bundle whose DSL-owned seed facts
/// came from a generated MeerkatMachine-owned snapshot.
#[derive(Debug, Clone)]
pub struct InputStatePersistenceRecord {
    bundle: StoredInputState,
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
        Ok(Self { bundle })
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
#[derive(Debug, Clone)]
pub struct InputState {
    pub input_id: InputId,
    pub history: Vec<InputStateHistoryEntry>,
    pub updated_at: DateTime<Utc>,
    pub policy: Option<PolicySnapshot>,
    /// Runtime-stamped run semantics captured at admission and persisted so
    /// recovery does not reclassify execution kind from payload shape.
    pub runtime_semantics: Option<RuntimeInputSemantics>,
    pub durability: Option<crate::input::InputDurability>,
    pub idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    pub recovery_count: u32,
    pub reconstruction_source: Option<ReconstructionSource>,
    /// Exact directed-terminal retry carrier, when this input came from the
    /// tracked cross-host flow lane.
    pub(crate) interaction_terminal_outbox: Option<InteractionTerminalOutbox>,
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
            durability: None,
            idempotency_key: None,
            recovery_count: 0,
            reconstruction_source: None,
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
// Field names, types, defaults, and `skip_serializing_if` markers are kept
// verbatim from the pre-5G/1 release. Since `InputState` no longer owns the
// three DSL-authoritative fields, serialization flows through
// [`StoredInputState`] where shell + seed can be bundled into the wire
// struct.

#[derive(Serialize, Deserialize)]
struct InputStateSerde {
    stored_input_state_version: u32,
    input_id: InputId,
    current_state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<PolicySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_semantics: Option<RuntimeInputSemantics>,
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
        let helper = InputStateSerde {
            stored_input_state_version:
                meerkat_core::generated::session_persistence_version_authority::stored_input_state_version(
                ),
            input_id: self.state.input_id.clone(),
            current_state: self.seed.phase,
            policy: self.state.policy.clone(),
            runtime_semantics: self.state.runtime_semantics,
            terminal_outcome: self.seed.terminal_outcome.clone(),
            durability: self.state.durability,
            idempotency_key: self.state.idempotency_key.clone(),
            attempt_count: self.seed.attempt_count,
            recovery_count: self.state.recovery_count,
            history: self.state.history.clone(),
            reconstruction_source: self.state.reconstruction_source.clone(),
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
        if observed_stored_input_state_version == 3 && helper.interaction_terminal_outbox.is_some()
        {
            return Err(<D::Error as serde::de::Error>::custom(
                "stored input state v3 cannot carry a v4 interaction terminal outbox",
            ));
        }
        if let Some(outbox) = helper.interaction_terminal_outbox.as_ref() {
            outbox
                .validate()
                .map_err(<D::Error as serde::de::Error>::custom)?;
        }
        let state = InputState {
            input_id: helper.input_id,
            history: helper.history,
            updated_at: helper.updated_at,
            policy: helper.policy,
            runtime_semantics: helper.runtime_semantics,
            durability: helper.durability,
            idempotency_key: helper.idempotency_key,
            recovery_count: helper.recovery_count,
            reconstruction_source: helper.reconstruction_source,
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
    fn stored_input_state_v3_fixture_migrates_to_v4() {
        let fixture = include_str!("../tests/fixtures/stored_input_state_v3.json");
        let restored: StoredInputState =
            serde_json::from_str(fixture).expect("serialized v3 fixture must migrate");
        assert_eq!(restored.seed.phase, InputLifecycleState::Queued);
        assert_eq!(restored.seed.attempt_count, 2);
        assert_eq!(restored.state.recovery_count, 1);
        assert_eq!(restored.seed.admission_sequence, Some(17));
        assert_eq!(restored.seed.recovery_lane, Some(HandlingMode::Queue));
        assert!(restored.state.interaction_terminal_outbox.is_none());

        let migrated = serde_json::to_value(&restored).expect("serialize migrated v4 row");
        assert_eq!(
            migrated["stored_input_state_version"],
            meerkat_core::generated::session_persistence_version_authority::STORED_INPUT_STATE_VERSION,
        );
    }

    #[test]
    fn stored_input_state_unlisted_legacy_version_still_fails_closed() {
        let mut fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/stored_input_state_v3.json"))
                .expect("fixture json");
        for rejected in [2, 5] {
            fixture["stored_input_state_version"] = serde_json::json!(rejected);
            let error = serde_json::from_value::<StoredInputState>(fixture.clone())
                .expect_err("unlisted historical and future versions must fail closed");
            assert!(error.to_string().contains("expected current 4"));
        }
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
