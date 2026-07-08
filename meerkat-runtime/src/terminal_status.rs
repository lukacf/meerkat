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

use crate::identifiers::IdempotencyKey;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputTerminalOutcome, StoredInputState,
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
}
