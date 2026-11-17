//! Restart-first-class terminal-status evaluation over input-state witnesses.
//!
//! The durable truth for "did interaction X / run Y finish, and how?" is the
//! set of per-input [`StoredInputState`] bundles: the DSL-owned seed carries
//! `phase`, `last_run_id`, `terminal_outcome`, and `attempt_count`, and the
//! runtime store commits those rows atomically at every machine lifecycle
//! boundary (they are never deleted). This module owns the single canonical,
//! pure evaluation used by BOTH witnesses — the live DSL-backed snapshot of a
//! registered session and the durable store rows of an unregistered one — so
//! the two sources cannot drift semantically.
//!
//! Honest limitation (run-status): an input that is re-staged to a later run
//! rebinds `seed.last_run_id`, so a crashed-and-retried run can legitimately
//! report [`RunTerminalStatus::NoDurableWitness`]. That is the durable truth;
//! callers must not treat it as `Failed`.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::{InputId, RunId};

use crate::completion::{CompletionClass, CompletionOutcome};
use crate::identifiers::IdempotencyKey;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputTerminalCompletionBatchKey,
    InputTerminalCompletionBatchRead, InputTerminalCompletionPhase,
    InputTerminalCompletionReadError, InputTerminalOutcome, StoredInputState,
    receipt_less_terminal,
};

/// Exactly-one lookup key for an interaction terminal-status query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionSelector {
    /// Look up by the canonical runtime input id.
    InputId(InputId),
    /// Look up by the caller-supplied idempotency key.
    IdempotencyKey(String),
}

/// Which witness answered a terminal-status query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalWitnessSource {
    /// The session is registered; facts were read from live DSL authority.
    LiveRuntime,
    /// The session is not registered; facts were read from the durably
    /// committed input-state rows in the runtime store.
    DurableStore,
}

/// Typed terminal-status report for a single interaction (input).
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionTerminalReport {
    pub input_id: InputId,
    /// DSL-owned lifecycle phase at the time of the witness.
    pub phase: InputLifecycleState,
    /// `None` => not yet terminal; `phase` says where the input is.
    pub terminal: Option<InputTerminalOutcome>,
    /// The run the input last contributed to (`seed.last_run_id`).
    pub resolving_run_id: Option<RunId>,
    pub attempt_count: u32,
    pub idempotency_key: Option<IdempotencyKey>,
    pub updated_at: DateTime<Utc>,
}

/// How a run resolved, projected from its terminal input witnesses.
#[derive(Debug, Clone, PartialEq)]
pub enum RunResolutionClass {
    /// At least one witness was consumed by the run.
    Consumed,
    /// No witness was consumed; the first abandoned witness (in admission
    /// order) supplies the typed cause.
    Abandoned { reason: InputAbandonReason },
    /// Only superseded/coalesced witnesses reference the run.
    Displaced,
}

/// Terminal status of a run, derived from durable input witnesses.
#[derive(Debug, Clone, PartialEq)]
pub enum RunTerminalStatus {
    /// At least one non-terminal witness is still bound to the run.
    InFlight,
    /// Every witness bound to the run is terminal.
    Resolved { outcome: RunResolutionClass },
    /// No input's `last_run_id` references the run. NOTE: re-staging rebinds
    /// `last_run_id`, so a retried run can legitimately land here.
    NoDurableWitness,
}

/// Terminal-status report for a run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunTerminalReport {
    pub run_id: RunId,
    pub status: RunTerminalStatus,
    /// Inputs whose `seed.last_run_id` references the run, in admission order.
    pub witnesses: Vec<InteractionTerminalReport>,
}

/// A report tagged with the witness source that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> {
    pub source: TerminalWitnessSource,
    pub report: T,
}

/// Which terminal transaction committed an input's terminal-completion batch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputTerminalReceiptScope {
    /// A terminal transaction of run `run_id` committed the batch.
    ///
    /// One run can commit more than one batch, so this does not mean the
    /// input shares a recipient set or an outcome with every input the run
    /// touched. An input joined into a running run at a live boundary (a
    /// request-only steer injected as turn context, or a retained durable
    /// join of a run that ends without a committed boundary) is finalized
    /// alone in its own `Run { run_id }` batch with
    /// `CompletedWithoutResult`, separate from the batch that carries the
    /// run's own result. A failed run's batch holds only the contributors the
    /// failure terminalized; requeued contributors get no receipt from it.
    Run { run_id: RunId },
    /// No run answered the input: the runtime terminalized it outside any
    /// run, in one runless batch (stop, reset, retire, destroy, executor
    /// attachment replacement, or cancellation of the still-queued input).
    ///
    /// A predecessor settlement also commits a runless batch for an input a
    /// recovered boundary consumed without staging its receipt: the input's
    /// terminal stays `Consumed`, and the outcome
    /// ([`CompletionOutcome::OutcomeUnknown`]) names the run that consumed
    /// it, because what that run did for the input is not known.
    RuntimeTermination,
}

/// The finalized terminal receipt of one input, read from the runtime's own
/// durable terminal-completion batch.
///
/// What it proves: the runtime finalized `input_id` in one terminal batch
/// together with exactly [`Self::recipient_input_ids`], all of which share
/// [`Self::outcome`], in a transaction of [`Self::scope`]. The recipient set
/// and the outcome come from the batch's canonical owner row, so every
/// recipient of one batch reads the same set and the same outcome. It does
/// not prove that the outcome answers every input a run touched, nor that a
/// run's result answers this input: see [`InputTerminalReceiptScope::Run`].
/// `input_id`, `terminal` and `attempt_count` are the target input's own
/// machine-owned facts.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InputTerminalReceipt {
    input_id: InputId,
    terminal: InputTerminalOutcome,
    attempt_count: u32,
    scope: InputTerminalReceiptScope,
    owner_input_id: InputId,
    recipient_input_ids: Vec<InputId>,
    outcome: CompletionOutcome,
}

impl InputTerminalReceipt {
    /// The input this receipt was read for.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }

    /// The input's machine-owned terminal outcome.
    #[must_use]
    pub fn terminal(&self) -> &InputTerminalOutcome {
        &self.terminal
    }

    /// Execution attempts the machine recorded for the input.
    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// The batch scope that produced the receipt.
    #[must_use]
    pub fn scope(&self) -> &InputTerminalReceiptScope {
        &self.scope
    }

    /// The run whose terminal transaction committed the input's batch, when
    /// a run did. For an input joined into that run at a live boundary this
    /// names the run, but the run's result is not this input's outcome.
    #[must_use]
    pub fn run_id(&self) -> Option<&RunId> {
        match &self.scope {
            InputTerminalReceiptScope::Run { run_id } => Some(run_id),
            InputTerminalReceiptScope::RuntimeTermination => None,
        }
    }

    /// The canonical owner input of the batch (the first recipient).
    #[must_use]
    pub fn owner_input_id(&self) -> &InputId {
        &self.owner_input_id
    }

    /// Every input finalized in the same batch, in canonical (input-id)
    /// order; all of them share [`Self::outcome`]. It always contains
    /// [`Self::input_id`]. More than one entry means one terminal transaction
    /// finalized several inputs with one outcome (for example queued inputs
    /// one run consumed together). It is the batch, not every input the run
    /// touched.
    #[must_use]
    pub fn recipient_input_ids(&self) -> &[InputId] {
        &self.recipient_input_ids
    }

    /// The batch's finalized public outcome, shared by every recipient.
    #[must_use]
    pub fn outcome(&self) -> &CompletionOutcome {
        &self.outcome
    }

    /// Take the batch's finalized public outcome.
    #[must_use]
    pub fn into_outcome(self) -> CompletionOutcome {
        self.outcome
    }
}

/// Terminal-receipt read for one input.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum InputTerminalReceiptRead {
    /// No finalized receipt yet. `terminal` is `Some` once the terminal
    /// transaction committed but its receipt is still being finalized.
    Pending {
        input_id: InputId,
        phase: InputLifecycleState,
        terminal: Option<InputTerminalOutcome>,
        last_run_id: Option<RunId>,
        attempt_count: u32,
    },
    /// The runtime finalized the input's terminal receipt.
    Finalized(Box<InputTerminalReceipt>),
    /// The input reached a terminal through a machine transition that stages
    /// no receipt: superseded or coalesced by a later admission, consumed on
    /// accept, cancelled by member-host boot revival, or abandoned at the
    /// stage-attempt cap after a failed batch start. No run result exists
    /// for it. `last_run_id` is the last run the input was staged into, if
    /// any; that run did not answer it.
    TerminalWithoutReceipt {
        input_id: InputId,
        terminal: InputTerminalOutcome,
        last_run_id: Option<RunId>,
        attempt_count: u32,
    },
}

impl InputTerminalReceiptRead {
    /// The input this read describes.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        match self {
            Self::Pending { input_id, .. } | Self::TerminalWithoutReceipt { input_id, .. } => {
                input_id
            }
            Self::Finalized(receipt) => receipt.input_id(),
        }
    }

    /// Whether the read is final: a finalized receipt or a receipt-less
    /// terminal. Only `Pending` can still change.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        !matches!(self, Self::Pending { .. })
    }
}

/// The body-free summary of one input's finalized terminal receipt: every
/// identity of [`InputTerminalReceipt`], read and validated from the same
/// durable terminal-completion batch, and the class of its outcome instead
/// of the outcome itself.
///
/// It carries no run result, no callback call and no failure text, so no
/// destination is needed to read it: a reader the scoped destination rule
/// refuses the receipt (`ScopedResultWithheld`) can still prove that the
/// input was finalized, with which recipients and in which class. It is a
/// read, not a close receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputTerminalReceiptSummary {
    input_id: InputId,
    terminal: InputTerminalOutcome,
    attempt_count: u32,
    scope: InputTerminalReceiptScope,
    owner_input_id: InputId,
    recipient_input_ids: Vec<InputId>,
    class: CompletionClass,
}

impl InputTerminalReceiptSummary {
    /// The input this summary was read for.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        &self.input_id
    }

    /// The input's machine-owned terminal outcome.
    #[must_use]
    pub fn terminal(&self) -> &InputTerminalOutcome {
        &self.terminal
    }

    /// Execution attempts the machine recorded for the input.
    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// The batch scope that produced the receipt.
    #[must_use]
    pub fn scope(&self) -> &InputTerminalReceiptScope {
        &self.scope
    }

    /// The run whose terminal transaction committed the input's batch, when
    /// a run did (see [`InputTerminalReceipt::run_id`]).
    #[must_use]
    pub fn run_id(&self) -> Option<&RunId> {
        match &self.scope {
            InputTerminalReceiptScope::Run { run_id } => Some(run_id),
            InputTerminalReceiptScope::RuntimeTermination => None,
        }
    }

    /// The canonical owner input of the batch (the first recipient).
    #[must_use]
    pub fn owner_input_id(&self) -> &InputId {
        &self.owner_input_id
    }

    /// Every input finalized in the same batch, in canonical (input-id)
    /// order (see [`InputTerminalReceipt::recipient_input_ids`]).
    #[must_use]
    pub fn recipient_input_ids(&self) -> &[InputId] {
        &self.recipient_input_ids
    }

    /// The class of the batch's finalized outcome, shared by every
    /// recipient.
    #[must_use]
    pub const fn class(&self) -> CompletionClass {
        self.class
    }
}

/// The body-free observation of one input's terminal receipt
/// ([`crate::MeerkatMachine::input_terminal_receipt_observation`]): the
/// variants of [`InputTerminalReceiptRead`], with a finalized receipt's
/// outcome replaced by its class. `Pending` and `TerminalWithoutReceipt`
/// carry exactly the read's facts, none of them a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputTerminalReceiptObservation {
    /// No finalized receipt yet. `terminal` is `Some` once the terminal
    /// transaction committed but its receipt is still being finalized.
    Pending {
        input_id: InputId,
        phase: InputLifecycleState,
        terminal: Option<InputTerminalOutcome>,
        last_run_id: Option<RunId>,
        attempt_count: u32,
    },
    /// The runtime finalized the input's terminal receipt.
    Finalized(Box<InputTerminalReceiptSummary>),
    /// The input reached a terminal through a machine transition that stages
    /// no receipt (see [`InputTerminalReceiptRead::TerminalWithoutReceipt`]).
    TerminalWithoutReceipt {
        input_id: InputId,
        terminal: InputTerminalOutcome,
        last_run_id: Option<RunId>,
        attempt_count: u32,
    },
}

impl InputTerminalReceiptObservation {
    /// The input this observation describes.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        match self {
            Self::Pending { input_id, .. } | Self::TerminalWithoutReceipt { input_id, .. } => {
                input_id
            }
            Self::Finalized(summary) => summary.input_id(),
        }
    }

    /// Whether the observation is final: a finalized receipt or a
    /// receipt-less terminal. Only `Pending` can still change.
    #[must_use]
    pub const fn is_resolved(&self) -> bool {
        !matches!(self, Self::Pending { .. })
    }
}

/// The body-free observation of a validated receipt read: every identity is
/// kept, and a finalized outcome is replaced by its class and dropped.
pub(crate) fn observe_input_terminal_receipt(
    read: InputTerminalReceiptRead,
) -> InputTerminalReceiptObservation {
    match read {
        InputTerminalReceiptRead::Pending {
            input_id,
            phase,
            terminal,
            last_run_id,
            attempt_count,
        } => InputTerminalReceiptObservation::Pending {
            input_id,
            phase,
            terminal,
            last_run_id,
            attempt_count,
        },
        InputTerminalReceiptRead::Finalized(receipt) => {
            let InputTerminalReceipt {
                input_id,
                terminal,
                attempt_count,
                scope,
                owner_input_id,
                recipient_input_ids,
                outcome,
            } = *receipt;
            InputTerminalReceiptObservation::Finalized(Box::new(InputTerminalReceiptSummary {
                input_id,
                terminal,
                attempt_count,
                scope,
                owner_input_id,
                recipient_input_ids,
                class: outcome.class(),
            }))
        }
        InputTerminalReceiptRead::TerminalWithoutReceipt {
            input_id,
            terminal,
            last_run_id,
            attempt_count,
        } => InputTerminalReceiptObservation::TerminalWithoutReceipt {
            input_id,
            terminal,
            last_run_id,
            attempt_count,
        },
    }
}

/// Result of [`crate::MeerkatMachine::wait_input_terminal_receipt`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum InputTerminalReceiptWait {
    /// The input's receipt resolved (`Finalized` or `TerminalWithoutReceipt`).
    /// For a directed input this is the receipt's finalization, independent
    /// of its interaction terminals' publication.
    Resolved(Sourced<InputTerminalReceiptRead>),
    /// The input is still `Pending` in the durable store and its session has
    /// no live registration to wait on (never registered, or unregistered
    /// while waiting). Carries that durable read. It can only advance once
    /// the session is registered again and recovery runs it.
    Detached(Sourced<InputTerminalReceiptRead>),
}

/// Classify the terminal receipt of `input_id` from rows that hold the target
/// and every row of its terminal-completion batch.
///
/// `Ok(None)` means the target is not among the rows. Batch validation and
/// the classification of receipt-less rows are shared with the public
/// completion reader ([`crate::input_state::receipt_less_terminal`]), so both
/// readers give the same verdict for the same row: a receipt-less machine
/// transition is `TerminalWithoutReceipt` here and
/// `RuntimeDriverError::InputTerminalWithoutReceipt` there, and every other
/// receipt-less terminal is repair-blocked or corrupt in both.
pub(crate) fn input_terminal_receipt_read(
    states: &[StoredInputState],
    input_id: &InputId,
) -> Result<Option<InputTerminalReceiptRead>, InputTerminalCompletionReadError> {
    match crate::input_state::input_terminal_completion_batch(states, input_id)? {
        InputTerminalCompletionBatchRead::TargetAbsent => Ok(None),
        InputTerminalCompletionBatchRead::NoReceipt { target } => {
            let seed = &target.seed;
            Ok(Some(match receipt_less_terminal(target)? {
                None => InputTerminalReceiptRead::Pending {
                    input_id: target.state.input_id.clone(),
                    phase: seed.phase,
                    terminal: None,
                    last_run_id: seed.last_run_id.clone(),
                    attempt_count: seed.attempt_count,
                },
                Some(terminal) => InputTerminalReceiptRead::TerminalWithoutReceipt {
                    input_id: target.state.input_id.clone(),
                    terminal,
                    last_run_id: seed.last_run_id.clone(),
                    attempt_count: seed.attempt_count,
                },
            }))
        }
        InputTerminalCompletionBatchRead::Batch { target, owner } => {
            let seed = &target.seed;
            match owner.phase {
                InputTerminalCompletionPhase::Pending => {
                    Ok(Some(InputTerminalReceiptRead::Pending {
                        input_id: target.state.input_id.clone(),
                        phase: seed.phase,
                        terminal: seed.terminal_outcome.clone(),
                        last_run_id: seed.last_run_id.clone(),
                        attempt_count: seed.attempt_count,
                    }))
                }
                InputTerminalCompletionPhase::Finalized { .. } => {
                    let corrupt = |reason: &str| {
                        InputTerminalCompletionReadError::Corrupt(reason.to_string())
                    };
                    let terminal = seed.terminal_outcome.clone().ok_or_else(|| {
                        corrupt("finalized terminal completion is bound to a non-terminal input")
                    })?;
                    let recipient_input_ids = owner.completion_input_ids.ok_or_else(|| {
                        corrupt("finalized terminal completion owner lost its recipient set")
                    })?;
                    let outcome = owner.outcome.ok_or_else(|| {
                        corrupt("finalized terminal completion owner lost outcome")
                    })?;
                    let scope = match owner.batch_key {
                        InputTerminalCompletionBatchKey::Run { run_id } => {
                            InputTerminalReceiptScope::Run { run_id }
                        }
                        InputTerminalCompletionBatchKey::RuntimeTermination { .. } => {
                            InputTerminalReceiptScope::RuntimeTermination
                        }
                    };
                    Ok(Some(InputTerminalReceiptRead::Finalized(Box::new(
                        InputTerminalReceipt {
                            input_id: target.state.input_id.clone(),
                            terminal,
                            attempt_count: seed.attempt_count,
                            scope,
                            owner_input_id: owner.owner_input_id,
                            recipient_input_ids,
                            outcome,
                        },
                    ))))
                }
            }
        }
    }
}

/// Project one input-state bundle into its typed interaction report.
///
/// Pure and I/O-free: the single canonical projection used by both the live
/// and the durable witness paths.
#[must_use]
pub fn interaction_report(bundle: &StoredInputState) -> InteractionTerminalReport {
    InteractionTerminalReport {
        input_id: bundle.state.input_id.clone(),
        phase: bundle.seed.phase,
        terminal: bundle.seed.terminal_outcome.clone(),
        resolving_run_id: bundle.seed.last_run_id.clone(),
        attempt_count: bundle.seed.attempt_count,
        idempotency_key: bundle.state.idempotency_key.clone(),
        updated_at: bundle.state.updated_at,
    }
}

/// Resolve an idempotency key to its input bundle by exact match on the
/// persisted shell key.
///
/// On the durable path this key IS the recovered authority fact: recovery
/// re-enters the machine-owned idempotency binding from this exact field, so
/// the durable witness and the live admission map cannot diverge.
#[must_use]
pub fn find_by_idempotency_key<'a>(
    inputs: &'a [StoredInputState],
    key: &str,
) -> Option<&'a StoredInputState> {
    inputs.iter().find(|stored| {
        stored
            .state
            .idempotency_key
            .as_ref()
            .is_some_and(|stored_key| stored_key.0 == key)
    })
}

/// Evaluate the terminal status of `run_id` over a set of input witnesses.
///
/// Witnesses are the inputs whose `seed.last_run_id` references the run,
/// ordered by admission sequence. Precedence for resolved runs:
/// `Consumed` > `Abandoned` (first witness in admission order supplies the
/// cause) > `Displaced`. An empty witness set is `NoDurableWitness`.
#[must_use]
pub fn evaluate_run(run_id: &RunId, inputs: &[StoredInputState]) -> RunTerminalReport {
    let mut witnesses: Vec<&StoredInputState> = inputs
        .iter()
        .filter(|stored| stored.seed.last_run_id.as_ref() == Some(run_id))
        .collect();
    // Admission order; inputs without an admission sequence sort last, then
    // input id keeps the order deterministic.
    witnesses.sort_by(|a, b| {
        let a_key = (
            a.seed.admission_sequence.is_none(),
            a.seed.admission_sequence,
        );
        let b_key = (
            b.seed.admission_sequence.is_none(),
            b.seed.admission_sequence,
        );
        a_key
            .cmp(&b_key)
            .then_with(|| a.state.input_id.0.cmp(&b.state.input_id.0))
    });

    let status = if witnesses.is_empty() {
        RunTerminalStatus::NoDurableWitness
    } else if witnesses
        .iter()
        .any(|stored| stored.seed.terminal_outcome.is_none())
    {
        RunTerminalStatus::InFlight
    } else if witnesses.iter().any(|stored| {
        matches!(
            stored.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        )
    }) {
        RunTerminalStatus::Resolved {
            outcome: RunResolutionClass::Consumed,
        }
    } else if let Some(reason) =
        witnesses
            .iter()
            .find_map(|stored| match &stored.seed.terminal_outcome {
                Some(InputTerminalOutcome::Abandoned { reason }) => Some(reason.clone()),
                _ => None,
            })
    {
        RunTerminalStatus::Resolved {
            outcome: RunResolutionClass::Abandoned { reason },
        }
    } else {
        RunTerminalStatus::Resolved {
            outcome: RunResolutionClass::Displaced,
        }
    };

    RunTerminalReport {
        run_id: run_id.clone(),
        status,
        witnesses: witnesses.into_iter().map(interaction_report).collect(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::input_state::{InputState, InputStateSeed};

    fn witness(
        run_id: Option<&RunId>,
        terminal: Option<InputTerminalOutcome>,
        admission_sequence: Option<u64>,
        idempotency_key: Option<&str>,
    ) -> StoredInputState {
        let input_id = InputId::new();
        let mut state = InputState::new_accepted(input_id.clone());
        state.idempotency_key = idempotency_key.map(IdempotencyKey::new);
        let phase = match &terminal {
            None => InputLifecycleState::Staged,
            Some(InputTerminalOutcome::Consumed) => InputLifecycleState::Consumed,
            Some(InputTerminalOutcome::Superseded { .. }) => InputLifecycleState::Superseded,
            Some(InputTerminalOutcome::Coalesced { .. }) => InputLifecycleState::Coalesced,
            Some(InputTerminalOutcome::Abandoned { .. }) => InputLifecycleState::Abandoned,
        };
        StoredInputState {
            state,
            seed: InputStateSeed {
                phase,
                last_run_id: run_id.cloned(),
                last_boundary_sequence: None,
                admission_sequence,
                terminal_outcome: terminal,
                attempt_count: 1,
                recovery_lane: None,
            },
        }
    }

    fn abandoned(reason: InputAbandonReason) -> Option<InputTerminalOutcome> {
        Some(InputTerminalOutcome::Abandoned { reason })
    }

    #[test]
    fn consumed_takes_precedence_over_abandoned_and_displaced() {
        let run_id = RunId::new();
        let inputs = vec![
            witness(
                Some(&run_id),
                abandoned(InputAbandonReason::Cancelled),
                Some(1),
                None,
            ),
            witness(
                Some(&run_id),
                Some(InputTerminalOutcome::Consumed),
                Some(2),
                None,
            ),
            witness(
                Some(&run_id),
                Some(InputTerminalOutcome::Superseded {
                    superseded_by: InputId::new(),
                }),
                Some(3),
                None,
            ),
        ];
        let report = evaluate_run(&run_id, &inputs);
        assert_eq!(
            report.status,
            RunTerminalStatus::Resolved {
                outcome: RunResolutionClass::Consumed
            }
        );
        assert_eq!(report.witnesses.len(), 3);
    }

    #[test]
    fn abandoned_cause_is_first_abandoned_witness_in_admission_order() {
        let run_id = RunId::new();
        let inputs = vec![
            witness(
                Some(&run_id),
                abandoned(InputAbandonReason::Stopped),
                Some(9),
                None,
            ),
            witness(
                Some(&run_id),
                abandoned(InputAbandonReason::Cancelled),
                Some(2),
                None,
            ),
        ];
        let report = evaluate_run(&run_id, &inputs);
        assert_eq!(
            report.status,
            RunTerminalStatus::Resolved {
                outcome: RunResolutionClass::Abandoned {
                    reason: InputAbandonReason::Cancelled
                }
            },
            "the FIRST abandoned witness in admission order supplies the cause"
        );
    }

    #[test]
    fn all_superseded_or_coalesced_is_displaced() {
        let run_id = RunId::new();
        let inputs = vec![
            witness(
                Some(&run_id),
                Some(InputTerminalOutcome::Superseded {
                    superseded_by: InputId::new(),
                }),
                Some(1),
                None,
            ),
            witness(
                Some(&run_id),
                Some(InputTerminalOutcome::Coalesced {
                    aggregate_id: InputId::new(),
                }),
                Some(2),
                None,
            ),
        ];
        let report = evaluate_run(&run_id, &inputs);
        assert_eq!(
            report.status,
            RunTerminalStatus::Resolved {
                outcome: RunResolutionClass::Displaced
            }
        );
    }

    #[test]
    fn one_non_terminal_witness_is_in_flight() {
        let run_id = RunId::new();
        let inputs = vec![
            witness(
                Some(&run_id),
                Some(InputTerminalOutcome::Consumed),
                Some(1),
                None,
            ),
            witness(Some(&run_id), None, Some(2), None),
        ];
        let report = evaluate_run(&run_id, &inputs);
        assert_eq!(report.status, RunTerminalStatus::InFlight);
    }

    #[test]
    fn empty_witness_set_is_no_durable_witness() {
        let run_id = RunId::new();
        let other_run = RunId::new();
        let inputs = vec![witness(
            Some(&other_run),
            Some(InputTerminalOutcome::Consumed),
            Some(1),
            None,
        )];
        let report = evaluate_run(&run_id, &inputs);
        assert_eq!(report.status, RunTerminalStatus::NoDurableWitness);
        assert!(report.witnesses.is_empty());
    }

    #[test]
    fn find_by_idempotency_key_is_exact_match_only() {
        let run_id = RunId::new();
        let inputs = vec![
            witness(
                Some(&run_id),
                Some(InputTerminalOutcome::Consumed),
                Some(1),
                Some("interaction-1"),
            ),
            witness(Some(&run_id), None, Some(2), None),
        ];
        assert!(find_by_idempotency_key(&inputs, "interaction-1").is_some());
        assert!(
            find_by_idempotency_key(&inputs, "interaction").is_none(),
            "prefix must not match"
        );
        assert!(
            find_by_idempotency_key(&inputs, "interaction-12").is_none(),
            "superstring must not match"
        );
    }

    /// Every terminal the machine can produce, with and without a run and on
    /// a migrated 0.8.10 row, classifies the same way in the receipt reader
    /// and the exact completion reader when the row carries no receipt.
    #[test]
    fn receipt_less_rows_classify_alike_in_both_readers() {
        #[derive(Debug, PartialEq)]
        enum Verdict {
            WithoutReceipt,
            RepairBlocked,
            Corrupt,
        }
        use Verdict::{Corrupt, RepairBlocked, WithoutReceipt};
        let run_id = RunId::new();
        let displaced_by = InputId::new();
        let abandoned = |reason| InputTerminalOutcome::Abandoned { reason };
        let cases = [
            (
                InputTerminalOutcome::Superseded {
                    superseded_by: displaced_by.clone(),
                },
                false,
                WithoutReceipt,
                WithoutReceipt,
            ),
            (
                InputTerminalOutcome::Coalesced {
                    aggregate_id: displaced_by.clone(),
                },
                false,
                WithoutReceipt,
                WithoutReceipt,
            ),
            (
                InputTerminalOutcome::Consumed,
                false,
                WithoutReceipt,
                RepairBlocked,
            ),
            (InputTerminalOutcome::Consumed, true, Corrupt, RepairBlocked),
            (
                abandoned(InputAbandonReason::Cancelled),
                true,
                WithoutReceipt,
                RepairBlocked,
            ),
            (
                abandoned(InputAbandonReason::MaxAttemptsExhausted { attempts: 3 }),
                true,
                WithoutReceipt,
                RepairBlocked,
            ),
            (
                abandoned(InputAbandonReason::Retired),
                false,
                Corrupt,
                RepairBlocked,
            ),
            (
                abandoned(InputAbandonReason::Reset),
                false,
                Corrupt,
                RepairBlocked,
            ),
            (
                abandoned(InputAbandonReason::Stopped),
                false,
                Corrupt,
                RepairBlocked,
            ),
            (
                abandoned(InputAbandonReason::Destroyed),
                false,
                Corrupt,
                RepairBlocked,
            ),
            (
                abandoned(InputAbandonReason::NeverExecuted),
                true,
                Corrupt,
                RepairBlocked,
            ),
        ];
        for (terminal, ran, current, migrated) in cases {
            for (unavailable, expected) in [(false, &current), (true, &migrated)] {
                let mut row = witness(ran.then_some(&run_id), Some(terminal.clone()), None, None);
                row.state.terminal_completion_unavailable = unavailable;
                let input_id = row.state.input_id.clone();
                let rows = std::slice::from_ref(&row);
                let receipt = match input_terminal_receipt_read(rows, &input_id) {
                    Ok(Some(InputTerminalReceiptRead::TerminalWithoutReceipt {
                        terminal: read,
                        last_run_id,
                        ..
                    })) => {
                        assert_eq!(read, terminal);
                        assert_eq!(last_run_id.as_ref(), ran.then_some(&run_id));
                        WithoutReceipt
                    }
                    Err(InputTerminalCompletionReadError::MigratedReceiptUnavailable) => {
                        RepairBlocked
                    }
                    Err(InputTerminalCompletionReadError::Corrupt(_)) => Corrupt,
                    other => panic!("unexpected receipt read {other:?} for {terminal:?}"),
                };
                let completion =
                    match crate::input_state::input_terminal_completion_outcome(rows, &input_id) {
                        Err(InputTerminalCompletionReadError::TerminalWithoutReceipt {
                            input_id: read_id,
                            terminal: read,
                        }) => {
                            assert_eq!(read_id, input_id);
                            assert_eq!(read, terminal);
                            WithoutReceipt
                        }
                        Err(InputTerminalCompletionReadError::MigratedReceiptUnavailable) => {
                            RepairBlocked
                        }
                        Err(InputTerminalCompletionReadError::Corrupt(_)) => Corrupt,
                        other => panic!("unexpected completion read {other:?} for {terminal:?}"),
                    };
                assert_eq!(
                    &receipt, expected,
                    "{terminal:?} ran={ran} migrated={unavailable}"
                );
                assert_eq!(
                    receipt, completion,
                    "both readers agree for {terminal:?} ran={ran} migrated={unavailable}"
                );
            }
        }
    }

    fn failure() -> meerkat_core::TurnErrorMetadata {
        meerkat_core::TurnErrorMetadata::terminal(
            meerkat_core::TurnTerminalCauseKind::FatalFailure,
            meerkat_core::TurnTerminalOutcome::Failed,
            "failure-text-marker",
        )
    }

    fn pending_call(marker: &str) -> meerkat_core::error::PendingCallbackToolCall {
        meerkat_core::error::PendingCallbackToolCall {
            tool_use_id: format!("call-id-marker-{marker}"),
            tool_name: format!("tool-name-marker-{marker}"),
            args: serde_json::json!({ "argument": format!("argument-marker-{marker}") }),
            settlement_failures: Vec::new(),
            dedupe_key: None,
        }
    }

    /// Every completion outcome has its body-free class. The two callback
    /// outcomes share one, so the class does not tell one pending call from
    /// several; no class is the pending (not yet finalized) state.
    #[test]
    fn every_completion_outcome_has_its_body_free_class() {
        let result: meerkat_core::RunResult = serde_json::from_value(serde_json::json!({
            "text": "result-marker",
            "session_id": meerkat_core::types::SessionId::new(),
            "usage": meerkat_core::Usage::default(),
            "turns": 1,
            "tool_calls": 0,
        }))
        .expect("minimal RunResult");
        let one = pending_call("one");
        let cases = [
            (
                CompletionOutcome::Completed(Box::new(result)),
                CompletionClass::Completed,
            ),
            (
                CompletionOutcome::CompletedWithoutResult,
                CompletionClass::CompletedWithoutResult,
            ),
            (
                CompletionOutcome::CallbackPending {
                    tool_use_id: one.tool_use_id,
                    tool_name: one.tool_name,
                    args: one.args,
                    dedupe_key: None,
                },
                CompletionClass::CallbackPending,
            ),
            (
                CompletionOutcome::CallbackBatchPending {
                    pending_tool_calls: vec![pending_call("two"), pending_call("three")],
                },
                CompletionClass::CallbackPending,
            ),
            (CompletionOutcome::Cancelled, CompletionClass::Cancelled),
            (
                CompletionOutcome::Abandoned {
                    reason: "reason-marker".into(),
                    error: failure(),
                },
                CompletionClass::Abandoned,
            ),
            (
                CompletionOutcome::AbandonedWithError {
                    reason: "reason-marker".into(),
                    error: failure(),
                },
                CompletionClass::AbandonedWithError,
            ),
            (
                CompletionOutcome::CompletedWithFinalizationFailure { error: failure() },
                CompletionClass::CompletedWithFinalizationFailure,
            ),
            (
                CompletionOutcome::RuntimeTerminated {
                    reason: "reason-marker".into(),
                    error: failure(),
                },
                CompletionClass::RuntimeTerminated,
            ),
            (
                CompletionOutcome::OutcomeUnknown {
                    run_id: meerkat_core::lifecycle::RunId::new(),
                },
                CompletionClass::OutcomeUnknown,
            ),
        ];
        for (outcome, class) in cases {
            assert_eq!(outcome.class(), class, "{outcome:?}");
        }
    }

    /// A receipt read projected without its outcome: a finalized receipt
    /// keeps every identity and replaces its outcome by the outcome's class,
    /// pending and receipt-less reads are copied as they are, and nothing of
    /// a result, a callback call or a failure text remains.
    #[test]
    fn the_observation_keeps_every_identity_and_drops_the_outcome() {
        let input_id = InputId::new();
        let owner = InputId::new();
        let run_id = RunId::new();
        let pending = |terminal: Option<InputTerminalOutcome>| InputTerminalReceiptRead::Pending {
            input_id: input_id.clone(),
            phase: InputLifecycleState::Consumed,
            terminal,
            last_run_id: Some(run_id.clone()),
            attempt_count: 2,
        };
        for terminal in [None, Some(InputTerminalOutcome::Consumed)] {
            let observed = observe_input_terminal_receipt(pending(terminal.clone()));
            assert_eq!(
                observed,
                InputTerminalReceiptObservation::Pending {
                    input_id: input_id.clone(),
                    phase: InputLifecycleState::Consumed,
                    terminal,
                    last_run_id: Some(run_id.clone()),
                    attempt_count: 2,
                }
            );
            assert!(!observed.is_resolved(), "pending stays pending");
        }

        let receipt_less =
            observe_input_terminal_receipt(InputTerminalReceiptRead::TerminalWithoutReceipt {
                input_id: input_id.clone(),
                terminal: InputTerminalOutcome::Coalesced {
                    aggregate_id: owner.clone(),
                },
                last_run_id: None,
                attempt_count: 1,
            });
        assert_eq!(
            receipt_less,
            InputTerminalReceiptObservation::TerminalWithoutReceipt {
                input_id: input_id.clone(),
                terminal: InputTerminalOutcome::Coalesced {
                    aggregate_id: owner.clone(),
                },
                last_run_id: None,
                attempt_count: 1,
            }
        );

        let finalized = |outcome: CompletionOutcome| {
            InputTerminalReceiptRead::Finalized(Box::new(InputTerminalReceipt {
                input_id: input_id.clone(),
                terminal: InputTerminalOutcome::Consumed,
                attempt_count: 3,
                scope: InputTerminalReceiptScope::Run {
                    run_id: run_id.clone(),
                },
                owner_input_id: owner.clone(),
                recipient_input_ids: vec![owner.clone(), input_id.clone()],
                outcome,
            }))
        };
        let outcomes = [
            CompletionOutcome::Completed(Box::new(
                serde_json::from_value(serde_json::json!({
                    "text": "result-marker",
                    "session_id": meerkat_core::types::SessionId::new(),
                    "usage": meerkat_core::Usage::default(),
                    "turns": 1,
                    "tool_calls": 0,
                }))
                .expect("minimal RunResult"),
            )),
            CompletionOutcome::CallbackBatchPending {
                pending_tool_calls: vec![pending_call("batch")],
            },
            CompletionOutcome::AbandonedWithError {
                reason: "reason-marker".into(),
                error: failure(),
            },
        ];
        for outcome in outcomes {
            let class = outcome.class();
            let observed = observe_input_terminal_receipt(finalized(outcome));
            let InputTerminalReceiptObservation::Finalized(summary) = &observed else {
                panic!("a finalized receipt is observed finalized: {observed:?}")
            };
            assert!(observed.is_resolved());
            assert_eq!(summary.input_id(), &input_id);
            assert_eq!(summary.terminal(), &InputTerminalOutcome::Consumed);
            assert_eq!(summary.attempt_count(), 3);
            assert_eq!(summary.run_id(), Some(&run_id));
            assert_eq!(summary.owner_input_id(), &owner);
            assert_eq!(
                summary.recipient_input_ids(),
                &[owner.clone(), input_id.clone()]
            );
            assert_eq!(summary.class(), class);
            let text = format!("{observed:?}");
            for marker in [
                "result-marker",
                "call-id-marker",
                "tool-name-marker",
                "argument-marker",
                "reason-marker",
                "failure-text-marker",
            ] {
                assert!(!text.contains(marker), "{marker} left in {text}");
            }
        }
    }

    #[test]
    fn interaction_report_projects_seed_and_shell_facts() {
        let run_id = RunId::new();
        let stored = witness(
            Some(&run_id),
            Some(InputTerminalOutcome::Consumed),
            Some(7),
            Some("key-7"),
        );
        let report = interaction_report(&stored);
        assert_eq!(report.input_id, stored.state.input_id);
        assert_eq!(report.phase, InputLifecycleState::Consumed);
        assert_eq!(report.terminal, Some(InputTerminalOutcome::Consumed));
        assert_eq!(report.resolving_run_id, Some(run_id));
        assert_eq!(report.attempt_count, 1);
        assert_eq!(report.idempotency_key, Some(IdempotencyKey::new("key-7")));
    }

    /// The stored input rows of a run that never began executing, written by
    /// 0.9.15 (the tag `v0.9.15`): one input a committed round had applied,
    /// one still staged; the run failed with its executor stopped and its
    /// contributors were terminalized.
    const NEVER_BEGAN_EXECUTING_V0_9_15: &str =
        include_str!("../tests/fixtures/v0_9_15_inputs_of_a_run_that_never_began_executing.json");

    /// The stored input rows of a session whose admitted turn was refused
    /// for its reasoning effort level at its start, with an input queued
    /// behind it and one turn after it, written by 0.9.15 (the tag
    /// `v0.9.15`).
    const REFUSED_TURN_V0_9_15: &str =
        include_str!("../tests/fixtures/v0_9_15_inputs_of_a_turn_refused_for_its_level.json");

    /// The rows of the same session as this tree writes them: the refused
    /// turn ends alone, its input as never executed.
    const REFUSED_TURN_ENDING_ALONE: &str =
        include_str!("../tests/fixtures/inputs_of_a_turn_refused_for_its_level_ending_alone.json");

    /// What this tree's readers answer for every stored input row of
    /// `document` (the store's own serde form of the rows of one runtime):
    /// the stored terminal record, the exact completion reader's answer
    /// (which asks `receipt_less_terminal` about a row without a receipt),
    /// the terminal receipt reader's answer, and the public terminal class
    /// the machine projects. A refusal is part of the answer.
    fn stored_rows_read_back(document: &str) -> Vec<[String; 4]> {
        let rows: Vec<StoredInputState> =
            serde_json::from_str(document).expect("every stored row is read back");
        rows.iter()
            .map(|row| {
                let id = &row.state.input_id;
                let completion =
                    match crate::input_state::input_terminal_completion_outcome(&rows, id) {
                        Ok(Some(outcome)) => format!("{:?}", outcome.class()),
                        Ok(None) => "no receipt yet".to_string(),
                        Err(error) => format!("refused: {error}"),
                    };
                let receipt = match input_terminal_receipt_read(&rows, id) {
                    Ok(Some(InputTerminalReceiptRead::Finalized(receipt))) => {
                        match receipt.scope() {
                            InputTerminalReceiptScope::RuntimeTermination => {
                                "finalized, of a runtime termination".to_string()
                            }
                            _ => "finalized, of a run".to_string(),
                        }
                    }
                    Ok(Some(InputTerminalReceiptRead::Pending { .. })) => "pending".to_string(),
                    Ok(Some(InputTerminalReceiptRead::TerminalWithoutReceipt { .. })) => {
                        "terminal without a receipt".to_string()
                    }
                    Ok(None) => "absent".to_string(),
                    Err(error) => format!("refused: {error}"),
                };
                let public = match interaction_report(row).terminal {
                    None => "not terminal".to_string(),
                    Some(terminal) => {
                        match crate::meerkat_machine::resolve_input_public_terminal_outcome_projection(
                            id, &terminal,
                        ) {
                            Ok(Some(outcome)) => format!("{outcome:?}"),
                            Ok(None) => "none".to_string(),
                            Err(error) => format!("refused: {error}"),
                        }
                    }
                };
                [
                    serde_json::to_string(&row.seed.terminal_outcome).expect("a terminal record"),
                    completion,
                    receipt,
                    public,
                ]
            })
            .collect()
    }

    fn read_back(terminal: &str, completion: &str, receipt: &str, public: &str) -> [String; 4] {
        [terminal, completion, receipt, public].map(str::to_string)
    }

    /// An upgrade and a rollback meet each other's stored inputs. A turn
    /// refused for its reasoning effort level at its start ends its input,
    /// and the input queued behind it, as `stopped` in 0.9.15 (the receipt
    /// of a runtime termination), and its own input alone as
    /// `never_executed` (the receipt of its run) from the release that lets
    /// the refused turn end alone; `never_executed` is the reason 0.9.15
    /// itself stores for the inputs of a run that never began executing.
    /// This tree reads 0.9.15's rows of both kinds, and its own: every row
    /// is read back, no reader refuses one, and every refused or stopped
    /// input is projected as the public outcome `Abandoned`. The fixture
    /// files hold the rows as each side stored them. The reverse reading
    /// (0.9.15's own readers over this tree's rows) was run at the tag and
    /// is not a test of this tree: it gave the answers this row holds for
    /// `REFUSED_TURN_ENDING_ALONE`.
    #[test]
    fn stored_inputs_of_a_refused_turn_are_read_across_0_9_15_and_the_turn_that_ends_alone() {
        let consumed = r#"{"outcome_type":"consumed"}"#;
        let never_executed = r#"{"outcome_type":"abandoned","reason":"never_executed"}"#;
        let stopped = r#"{"outcome_type":"abandoned","reason":"stopped"}"#;

        assert_eq!(
            stored_rows_read_back(NEVER_BEGAN_EXECUTING_V0_9_15),
            vec![
                read_back(consumed, "no receipt yet", "pending", "Completed"),
                read_back(never_executed, "no receipt yet", "pending", "Abandoned"),
            ],
            "0.9.15's rows of a run that never began executing: the applied input, the staged one"
        );
        assert_eq!(
            stored_rows_read_back(REFUSED_TURN_V0_9_15),
            vec![
                read_back(consumed, "Completed", "finalized, of a run", "Completed"),
                read_back(
                    stopped,
                    "RuntimeTerminated",
                    "finalized, of a runtime termination",
                    "Abandoned"
                ),
                read_back(
                    stopped,
                    "RuntimeTerminated",
                    "finalized, of a runtime termination",
                    "Abandoned"
                ),
                read_back(consumed, "Completed", "finalized, of a run", "Completed"),
            ],
            "0.9.15's rows: the session's first turn, the refused turn, the input queued \
             behind it, the next turn"
        );
        assert_eq!(
            stored_rows_read_back(REFUSED_TURN_ENDING_ALONE),
            vec![
                read_back(consumed, "Completed", "finalized, of a run", "Completed"),
                read_back(
                    never_executed,
                    "AbandonedWithError",
                    "finalized, of a run",
                    "Abandoned"
                ),
                read_back(consumed, "Completed", "finalized, of a run", "Completed"),
                read_back(consumed, "Completed", "finalized, of a run", "Completed"),
            ],
            "this tree's rows: the session's first turn, the refused turn, the input queued \
             behind it, the next turn"
        );

        // The rule is asked: the refused input without its receipt is not a
        // receipt-less terminal, so both readers refuse the row.
        let mut rows: serde_json::Value =
            serde_json::from_str(REFUSED_TURN_ENDING_ALONE).expect("the fixture is JSON");
        let refused = rows
            .as_array_mut()
            .expect("an array of rows")
            .iter_mut()
            .find(|row| row["terminal_outcome"]["reason"] == "never_executed")
            .expect("the refused turn's row");
        refused
            .as_object_mut()
            .expect("a row is an object")
            .remove("terminal_completion")
            .expect("the refused turn's row carries its receipt");
        let lost = "refused: v5 terminal input lost its exact completion receipt";
        assert_eq!(
            stored_rows_read_back(&rows.to_string())[1],
            read_back(never_executed, lost, lost, "Abandoned"),
            "the refused turn's row without its receipt"
        );
    }
}
