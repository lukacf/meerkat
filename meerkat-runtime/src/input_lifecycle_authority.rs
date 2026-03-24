//! Generated-authority module for the InputLifecycle machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all InputLifecycle state mutations flow through the machine authority.
//! Handwritten shell code calls [`InputLifecycleAuthority::apply`] and executes
//! returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/input_lifecycle.rs`:
//!
//! - 9 states: Accepted, Queued, Staged, Applied, AppliedPendingConsumption,
//!   Consumed, Superseded, Coalesced, Abandoned
//! - 9 inputs: QueueAccepted, StageForRun, RollbackStaged, MarkApplied,
//!   MarkAppliedPendingConsumption, Consume, Supersede, Coalesce, Abandon
//! - 3 fields: terminal_outcome, last_run_id, last_boundary_sequence
//! - 4 terminal states: Consumed, Superseded, Coalesced, Abandoned
//! - 4 effects: InputLifecycleNotice, RecordTerminalOutcome,
//!   RecordRunAssociation, RecordBoundarySequence

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::{InputId, RunId};

use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputStateHistoryEntry, InputTerminalOutcome,
};

// ---------------------------------------------------------------------------
// Typed input enum -- mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the InputLifecycle machine.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`InputLifecycleAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputLifecycleInput {
    /// Accepted -> Queued: input has been policy-resolved and is ready for queueing.
    QueueAccepted,
    /// Queued -> Staged: input is being staged for a specific run.
    StageForRun { run_id: RunId },
    /// Staged -> Queued: rollback on run failure.
    RollbackStaged,
    /// Staged -> Applied: the input's boundary primitive has been applied.
    MarkApplied { run_id: RunId },
    /// Applied -> AppliedPendingConsumption: boundary receipt confirms application.
    MarkAppliedPendingConsumption { boundary_sequence: u64 },
    /// AppliedPendingConsumption -> Consumed: run completed successfully.
    Consume,
    /// Queued -> Superseded: a newer input with the same supersession scope arrived.
    Supersede,
    /// Queued -> Coalesced: input was merged into an aggregate.
    Coalesce,
    /// Any non-terminal -> Abandoned: input was abandoned (retire/reset/destroy/cancel).
    Abandon { reason: InputAbandonReason },
    /// Accepted -> Consumed: shortcut for Ignore+OnAccept policy (no queue/run cycle).
    ConsumeOnAccept,
}

// ---------------------------------------------------------------------------
// Typed effect enum -- mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by InputLifecycle transitions.
///
/// Shell code receives these from [`InputLifecycleAuthority::apply`] and is
/// responsible for executing the side effects (e.g. emitting events, persisting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputLifecycleEffect {
    /// Notify observers of the new lifecycle state.
    InputLifecycleNotice { new_state: InputLifecycleState },
    /// Record the terminal outcome for this input.
    RecordTerminalOutcome { outcome: InputTerminalOutcome },
    /// Record the run association (which run touched this input).
    RecordRunAssociation { run_id: RunId },
    /// Record the boundary receipt sequence number.
    RecordBoundarySequence { boundary_sequence: u64 },
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the InputLifecycle authority.
#[derive(Debug)]
pub struct InputLifecycleTransition {
    /// The phase after the transition.
    pub next_phase: InputLifecycleState,
    /// Effects to be executed by shell code.
    pub effects: Vec<InputLifecycleEffect>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from the input lifecycle authority.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum InputLifecycleError {
    /// The transition is not valid from the current state.
    #[error("Invalid transition: {from:?} via {input} (current phase rejects this input)")]
    InvalidTransition {
        from: InputLifecycleState,
        input: String,
    },
    /// The input is already in a terminal state.
    #[error("Input is in terminal state {state:?}")]
    TerminalState { state: InputLifecycleState },
    /// A guard condition was not met.
    #[error("Guard failed: {guard} (from {from:?})")]
    GuardFailed {
        from: InputLifecycleState,
        guard: String,
    },
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

/// Canonical machine-owned fields for InputLifecycle.
///
/// These fields are owned exclusively by the authority and cannot be mutated
/// by handwritten shell code.
#[derive(Debug, Clone)]
struct InputLifecycleFields {
    terminal_outcome: Option<InputTerminalOutcome>,
    last_run_id: Option<RunId>,
    last_boundary_sequence: Option<u64>,
}

// ---------------------------------------------------------------------------
// Sealed mutator trait -- only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for InputLifecycle state mutation.
///
/// Only [`InputLifecycleAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for lifecycle state.
pub trait InputLifecycleMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(
        &mut self,
        input: InputLifecycleInput,
    ) -> Result<InputLifecycleTransition, InputLifecycleError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for InputLifecycle state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table. The authority OWNS the canonical state --
/// callers cannot get `&mut` access to the inner fields.
#[derive(Debug, Clone)]
pub struct InputLifecycleAuthority {
    /// Canonical phase.
    phase: InputLifecycleState,
    /// Canonical machine-owned fields.
    fields: InputLifecycleFields,
    /// State transition history.
    history: Vec<InputStateHistoryEntry>,
    /// Timestamp of last state change.
    updated_at: DateTime<Utc>,
}

impl sealed::Sealed for InputLifecycleAuthority {}

impl Default for InputLifecycleAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl InputLifecycleAuthority {
    /// Create a new authority in the Accepted state.
    pub fn new() -> Self {
        Self::new_at(Utc::now())
    }

    /// Create a new authority in the Accepted state with a caller-owned timestamp.
    ///
    /// Use this when the caller already captured a canonical `now` to ensure
    /// `updated_at` is consistent with sibling timestamps (e.g., `created_at`
    /// on the owning `InputState`).
    pub fn new_at(now: DateTime<Utc>) -> Self {
        Self {
            phase: InputLifecycleState::Accepted,
            fields: InputLifecycleFields {
                terminal_outcome: None,
                last_run_id: None,
                last_boundary_sequence: None,
            },
            history: Vec::new(),
            updated_at: now,
        }
    }

    /// Create an authority initialized to a specific phase (for recovery).
    pub fn with_phase(phase: InputLifecycleState) -> Self {
        Self {
            phase,
            fields: InputLifecycleFields {
                terminal_outcome: None,
                last_run_id: None,
                last_boundary_sequence: None,
            },
            history: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    /// Restore an authority from persisted state (for crash recovery).
    ///
    /// This reconstructs the authority from a previously-serialized snapshot,
    /// including all canonical fields and history.
    pub fn restore(
        phase: InputLifecycleState,
        terminal_outcome: Option<InputTerminalOutcome>,
        last_run_id: Option<RunId>,
        last_boundary_sequence: Option<u64>,
        history: Vec<InputStateHistoryEntry>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            phase,
            fields: InputLifecycleFields {
                terminal_outcome,
                last_run_id,
                last_boundary_sequence,
            },
            history,
            updated_at,
        }
    }

    /// Current phase (read from canonical state).
    pub fn phase(&self) -> InputLifecycleState {
        self.phase
    }

    /// Whether the current phase is terminal.
    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal()
    }

    /// Current terminal outcome (if in a terminal state).
    pub fn terminal_outcome(&self) -> Option<&InputTerminalOutcome> {
        self.fields.terminal_outcome.as_ref()
    }

    /// Last run ID that touched this input.
    pub fn last_run_id(&self) -> Option<&RunId> {
        self.fields.last_run_id.as_ref()
    }

    /// Last boundary sequence number.
    pub fn last_boundary_sequence(&self) -> Option<u64> {
        self.fields.last_boundary_sequence
    }

    /// State transition history.
    pub fn history(&self) -> &[InputStateHistoryEntry] {
        &self.history
    }

    /// Timestamp of last state change.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Check if a transition is legal without applying it.
    pub fn can_accept(&self, input: &InputLifecycleInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Require that the authority is in one of the given phases.
    pub fn require_phase(
        &self,
        allowed: &[InputLifecycleState],
    ) -> Result<(), InputLifecycleError> {
        if allowed.contains(&self.phase) {
            Ok(())
        } else {
            Err(InputLifecycleError::InvalidTransition {
                from: self.phase,
                input: format!("require_phase({allowed:?})"),
            })
        }
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &InputLifecycleInput,
    ) -> Result<
        (
            InputLifecycleState,
            InputLifecycleFields,
            Vec<InputLifecycleEffect>,
        ),
        InputLifecycleError,
    > {
        #[allow(clippy::enum_glob_use)]
        use InputLifecycleInput::*;
        #[allow(clippy::enum_glob_use)]
        use InputLifecycleState::*;

        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Terminal states reject ALL inputs.
        if phase.is_terminal() {
            return Err(InputLifecycleError::TerminalState { state: phase });
        }

        let next_phase = match (phase, input) {
            // QueueAccepted: Accepted -> Queued
            (Accepted, QueueAccepted) => {
                effects.push(InputLifecycleEffect::InputLifecycleNotice { new_state: Queued });
                Queued
            }

            // StageForRun: Queued -> Staged
            (Queued, StageForRun { run_id }) => {
                fields.last_run_id = Some(run_id.clone());
                effects.push(InputLifecycleEffect::InputLifecycleNotice { new_state: Staged });
                effects.push(InputLifecycleEffect::RecordRunAssociation {
                    run_id: run_id.clone(),
                });
                Staged
            }

            // RollbackStaged: Staged -> Queued
            (Staged, RollbackStaged) => {
                effects.push(InputLifecycleEffect::InputLifecycleNotice { new_state: Queued });
                Queued
            }

            // MarkApplied: Staged -> Applied (guard: matches_last_run)
            (Staged, MarkApplied { run_id }) => {
                if self.fields.last_run_id.as_ref() != Some(run_id) {
                    return Err(InputLifecycleError::GuardFailed {
                        from: phase,
                        guard: format!(
                            "matches_last_run: expected {:?}, got {run_id:?}",
                            self.fields.last_run_id
                        ),
                    });
                }
                effects.push(InputLifecycleEffect::InputLifecycleNotice { new_state: Applied });
                Applied
            }

            // MarkAppliedPendingConsumption: Applied -> AppliedPendingConsumption
            (Applied, MarkAppliedPendingConsumption { boundary_sequence }) => {
                fields.last_boundary_sequence = Some(*boundary_sequence);
                effects.push(InputLifecycleEffect::InputLifecycleNotice {
                    new_state: AppliedPendingConsumption,
                });
                effects.push(InputLifecycleEffect::RecordBoundarySequence {
                    boundary_sequence: *boundary_sequence,
                });
                AppliedPendingConsumption
            }

            // Consume: AppliedPendingConsumption -> Consumed
            (AppliedPendingConsumption, Consume) => {
                let outcome = InputTerminalOutcome::Consumed;
                fields.terminal_outcome = Some(outcome.clone());
                effects.push(InputLifecycleEffect::InputLifecycleNotice {
                    new_state: Consumed,
                });
                effects.push(InputLifecycleEffect::RecordTerminalOutcome { outcome });
                Consumed
            }

            // Supersede: Queued -> Superseded
            (Queued, Supersede) => {
                let outcome = InputTerminalOutcome::Superseded {
                    superseded_by: InputId::new(), // placeholder, caller sets via set_terminal_outcome
                };
                fields.terminal_outcome = Some(outcome.clone());
                effects.push(InputLifecycleEffect::InputLifecycleNotice {
                    new_state: Superseded,
                });
                effects.push(InputLifecycleEffect::RecordTerminalOutcome { outcome });
                Superseded
            }

            // Coalesce: Queued -> Coalesced
            (Queued, Coalesce) => {
                let outcome = InputTerminalOutcome::Coalesced {
                    aggregate_id: InputId::new(), // placeholder, caller sets via set_terminal_outcome
                };
                fields.terminal_outcome = Some(outcome.clone());
                effects.push(InputLifecycleEffect::InputLifecycleNotice {
                    new_state: Coalesced,
                });
                effects.push(InputLifecycleEffect::RecordTerminalOutcome { outcome });
                Coalesced
            }

            // Abandon: any non-terminal -> Abandoned
            (
                Accepted | Queued | Staged | Applied | AppliedPendingConsumption,
                Abandon { reason },
            ) => {
                let outcome = InputTerminalOutcome::Abandoned {
                    reason: reason.clone(),
                };
                fields.terminal_outcome = Some(outcome.clone());
                effects.push(InputLifecycleEffect::InputLifecycleNotice {
                    new_state: Abandoned,
                });
                effects.push(InputLifecycleEffect::RecordTerminalOutcome { outcome });
                Abandoned
            }

            // ConsumeOnAccept: Accepted -> Consumed (Ignore+OnAccept shortcut)
            (Accepted, ConsumeOnAccept) => {
                let outcome = InputTerminalOutcome::Consumed;
                fields.terminal_outcome = Some(outcome.clone());
                effects.push(InputLifecycleEffect::InputLifecycleNotice {
                    new_state: Consumed,
                });
                effects.push(InputLifecycleEffect::RecordTerminalOutcome { outcome });
                Consumed
            }

            // All other combinations are illegal.
            _ => {
                return Err(InputLifecycleError::InvalidTransition {
                    from: phase,
                    input: format!("{input:?}"),
                });
            }
        };

        Ok((next_phase, fields, effects))
    }

    /// Set the terminal outcome after a transition.
    ///
    /// Used by shell code for Superseded/Coalesced which need caller-provided
    /// data (superseded_by / aggregate_id) that the authority's transition
    /// doesn't know at evaluate-time.
    pub fn set_terminal_outcome(&mut self, outcome: InputTerminalOutcome) {
        self.fields.terminal_outcome = Some(outcome);
    }

    /// Stamp receipt metadata onto the authority state.
    ///
    /// Used by the store layer after it generates an authoritative receipt
    /// with a sequence number. This is NOT a lifecycle transition -- it's
    /// operational metadata enrichment by the persistence layer.
    pub fn stamp_receipt_metadata(&mut self, run_id: RunId, boundary_sequence: u64) {
        self.fields.last_run_id = Some(run_id);
        self.fields.last_boundary_sequence = Some(boundary_sequence);
    }
}

impl InputLifecycleMutator for InputLifecycleAuthority {
    fn apply(
        &mut self,
        input: InputLifecycleInput,
    ) -> Result<InputLifecycleTransition, InputLifecycleError> {
        let from = self.phase;
        let reason = format!("{input:?}");
        let (next_phase, next_fields, effects) = self.evaluate(&input)?;

        let now = Utc::now();

        // Record history.
        self.history.push(InputStateHistoryEntry {
            timestamp: now,
            from,
            to: next_phase,
            reason: Some(reason),
        });

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;
        self.updated_at = now;

        Ok(InputLifecycleTransition {
            next_phase,
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone,
    clippy::panic
)]
mod tests {
    use super::*;

    fn make_authority() -> InputLifecycleAuthority {
        InputLifecycleAuthority::new()
    }

    fn make_at_phase(phase: InputLifecycleState) -> InputLifecycleAuthority {
        InputLifecycleAuthority::with_phase(phase)
    }

    // ---- Happy path: full lifecycle ----

    #[test]
    fn accepted_to_queued() {
        let mut auth = make_authority();
        let t = auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Queued);
        assert_eq!(auth.phase(), InputLifecycleState::Queued);
        assert_eq!(auth.history().len(), 1);
        assert_eq!(auth.history()[0].from, InputLifecycleState::Accepted);
        assert_eq!(auth.history()[0].to, InputLifecycleState::Queued);
    }

    #[test]
    fn queued_to_staged() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let run_id = RunId::new();
        let t = auth
            .apply(InputLifecycleInput::StageForRun {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Staged);
        assert_eq!(auth.last_run_id(), Some(&run_id));
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, InputLifecycleEffect::RecordRunAssociation { .. }))
        );
    }

    #[test]
    fn staged_to_applied() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        let t = auth
            .apply(InputLifecycleInput::MarkApplied {
                run_id: run_id.clone(),
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Applied);
    }

    #[test]
    fn applied_to_applied_pending_consumption() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        let t = auth
            .apply(InputLifecycleInput::MarkAppliedPendingConsumption {
                boundary_sequence: 42,
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::AppliedPendingConsumption);
        assert_eq!(auth.last_boundary_sequence(), Some(42));
        assert!(t.effects.iter().any(|e| matches!(
            e,
            InputLifecycleEffect::RecordBoundarySequence {
                boundary_sequence: 42
            }
        )));
    }

    #[test]
    fn applied_pending_to_consumed() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 1,
        })
        .unwrap();
        let t = auth.apply(InputLifecycleInput::Consume).unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Consumed);
        assert!(auth.is_terminal());
        assert!(matches!(
            auth.terminal_outcome(),
            Some(InputTerminalOutcome::Consumed)
        ));
    }

    #[test]
    fn full_happy_path_history() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 1,
        })
        .unwrap();
        auth.apply(InputLifecycleInput::Consume).unwrap();
        assert_eq!(auth.history().len(), 5);
    }

    // ---- Staged rollback ----

    #[test]
    fn staged_to_queued_rollback() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        let t = auth.apply(InputLifecycleInput::RollbackStaged).unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Queued);
        assert_eq!(auth.phase(), InputLifecycleState::Queued);
    }

    // ---- MarkApplied guard: matches_last_run ----

    #[test]
    fn mark_applied_rejects_wrong_run_id() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        let wrong_run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        let result = auth.apply(InputLifecycleInput::MarkApplied {
            run_id: wrong_run_id,
        });
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InputLifecycleError::GuardFailed { .. }
        ));
        // Phase unchanged on failure.
        assert_eq!(auth.phase(), InputLifecycleState::Staged);
    }

    // ---- Hard rule: AppliedPendingConsumption -> Queued REJECTED ----

    #[test]
    fn applied_pending_to_queued_rejected() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 1,
        })
        .unwrap();

        let result = auth.apply(InputLifecycleInput::RollbackStaged);
        assert!(result.is_err());
        assert_eq!(auth.phase(), InputLifecycleState::AppliedPendingConsumption);
    }

    // ---- Terminal states reject ALL transitions ----

    #[test]
    fn consumed_rejects_all() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 1,
        })
        .unwrap();
        auth.apply(InputLifecycleInput::Consume).unwrap();

        let result = auth.apply(InputLifecycleInput::QueueAccepted);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InputLifecycleError::TerminalState { .. }
        ));
    }

    #[test]
    fn superseded_rejects_all() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::Supersede).unwrap();
        assert!(auth.is_terminal());

        let result = auth.apply(InputLifecycleInput::QueueAccepted);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InputLifecycleError::TerminalState { .. }
        ));
    }

    #[test]
    fn coalesced_rejects_all() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::Coalesce).unwrap();
        assert!(auth.is_terminal());

        let result = auth.apply(InputLifecycleInput::QueueAccepted);
        assert!(result.is_err());
    }

    #[test]
    fn abandoned_rejects_all() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::Abandon {
            reason: InputAbandonReason::Retired,
        })
        .unwrap();
        assert!(auth.is_terminal());

        let result = auth.apply(InputLifecycleInput::QueueAccepted);
        assert!(result.is_err());
    }

    // ---- Abandon from various states ----

    #[test]
    fn abandon_from_accepted() {
        let mut auth = make_authority();
        let t = auth
            .apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Retired,
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Abandoned);
        assert!(matches!(
            auth.terminal_outcome(),
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Retired,
            })
        ));
    }

    #[test]
    fn abandon_from_queued() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let t = auth
            .apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Reset,
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Abandoned);
    }

    #[test]
    fn abandon_from_staged() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        let t = auth
            .apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Destroyed,
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Abandoned);
    }

    #[test]
    fn abandon_from_applied() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        let t = auth
            .apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Cancelled,
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Abandoned);
    }

    #[test]
    fn abandon_from_applied_pending() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 1,
        })
        .unwrap();
        let t = auth
            .apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Retired,
            })
            .unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Abandoned);
    }

    // ---- ConsumeOnAccept (Ignore + OnAccept) ----

    #[test]
    fn consume_on_accept_from_accepted() {
        let mut auth = make_authority();
        let t = auth.apply(InputLifecycleInput::ConsumeOnAccept).unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Consumed);
        assert!(auth.is_terminal());
        assert!(matches!(
            auth.terminal_outcome(),
            Some(InputTerminalOutcome::Consumed)
        ));
    }

    #[test]
    fn consume_on_accept_from_queued_rejected() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let result = auth.apply(InputLifecycleInput::ConsumeOnAccept);
        assert!(result.is_err());
    }

    // ---- Invalid transitions ----

    #[test]
    fn accepted_to_staged_invalid() {
        let mut auth = make_authority();
        let result = auth.apply(InputLifecycleInput::StageForRun {
            run_id: RunId::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn accepted_to_applied_invalid() {
        let mut auth = make_authority();
        let result = auth.apply(InputLifecycleInput::MarkApplied {
            run_id: RunId::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn queued_to_applied_invalid() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let result = auth.apply(InputLifecycleInput::MarkApplied {
            run_id: RunId::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn queued_to_consumed_invalid() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let result = auth.apply(InputLifecycleInput::Consume);
        assert!(result.is_err());
    }

    // ---- Supersede / Coalesce ----

    #[test]
    fn supersede_from_queued() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let t = auth.apply(InputLifecycleInput::Supersede).unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Superseded);
        assert!(auth.is_terminal());
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, InputLifecycleEffect::RecordTerminalOutcome { .. }))
        );
    }

    #[test]
    fn supersede_from_accepted_rejected() {
        let mut auth = make_authority();
        let result = auth.apply(InputLifecycleInput::Supersede);
        assert!(result.is_err());
    }

    #[test]
    fn coalesce_from_queued() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let t = auth.apply(InputLifecycleInput::Coalesce).unwrap();
        assert_eq!(t.next_phase, InputLifecycleState::Coalesced);
        assert!(auth.is_terminal());
    }

    #[test]
    fn coalesce_from_accepted_rejected() {
        let mut auth = make_authority();
        let result = auth.apply(InputLifecycleInput::Coalesce);
        assert!(result.is_err());
    }

    // ---- Set terminal outcome ----

    #[test]
    fn set_terminal_outcome_superseded() {
        let mut auth = make_authority();
        let superseder = InputId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::Supersede).unwrap();
        auth.set_terminal_outcome(InputTerminalOutcome::Superseded {
            superseded_by: superseder.clone(),
        });
        match auth.terminal_outcome() {
            Some(InputTerminalOutcome::Superseded { superseded_by }) => {
                assert_eq!(superseded_by, &superseder);
            }
            other => panic!("expected Superseded, got {other:?}"),
        }
    }

    #[test]
    fn set_terminal_outcome_coalesced() {
        let mut auth = make_authority();
        let aggregate = InputId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::Coalesce).unwrap();
        auth.set_terminal_outcome(InputTerminalOutcome::Coalesced {
            aggregate_id: aggregate.clone(),
        });
        match auth.terminal_outcome() {
            Some(InputTerminalOutcome::Coalesced { aggregate_id }) => {
                assert_eq!(aggregate_id, &aggregate);
            }
            other => panic!("expected Coalesced, got {other:?}"),
        }
    }

    // ---- History recording ----

    #[test]
    fn history_records_reason() {
        let mut auth = make_authority();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        assert!(auth.history()[0].reason.is_some());
        assert!(
            auth.history()[0]
                .reason
                .as_deref()
                .is_some_and(|r| r.contains("QueueAccepted"))
        );
    }

    #[test]
    fn history_records_timestamps() {
        let mut auth = make_authority();
        let before = Utc::now();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        let after = Utc::now();
        assert!(auth.history()[0].timestamp >= before);
        assert!(auth.history()[0].timestamp <= after);
    }

    // ---- can_accept probing ----

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = make_authority();
        assert!(auth.can_accept(&InputLifecycleInput::QueueAccepted));
        assert!(!auth.can_accept(&InputLifecycleInput::Consume));
        // Phase is still Accepted -- no mutation.
        assert_eq!(auth.phase(), InputLifecycleState::Accepted);
    }

    // ---- require_phase ----

    #[test]
    fn require_phase_accepts_allowed() {
        let auth = make_authority();
        assert!(
            auth.require_phase(&[InputLifecycleState::Accepted, InputLifecycleState::Queued])
                .is_ok()
        );
    }

    #[test]
    fn require_phase_rejects_disallowed() {
        let auth = make_authority();
        let result = auth.require_phase(&[InputLifecycleState::Queued]);
        assert!(matches!(
            result,
            Err(InputLifecycleError::InvalidTransition { .. })
        ));
    }

    // ---- Phase unchanged on failure ----

    #[test]
    fn phase_unchanged_on_rejected_transition() {
        let mut auth = make_authority();
        let _ = auth.apply(InputLifecycleInput::Consume);
        assert_eq!(auth.phase(), InputLifecycleState::Accepted);
        assert!(auth.history().is_empty());
    }

    // ---- Restore from persisted state ----

    #[test]
    fn restore_preserves_all_fields() {
        let run_id = RunId::new();
        let auth = InputLifecycleAuthority::restore(
            InputLifecycleState::Applied,
            None,
            Some(run_id.clone()),
            Some(42),
            vec![InputStateHistoryEntry {
                timestamp: Utc::now(),
                from: InputLifecycleState::Accepted,
                to: InputLifecycleState::Queued,
                reason: Some("restored".into()),
            }],
            Utc::now(),
        );
        assert_eq!(auth.phase(), InputLifecycleState::Applied);
        assert_eq!(auth.last_run_id(), Some(&run_id));
        assert_eq!(auth.last_boundary_sequence(), Some(42));
        assert_eq!(auth.history().len(), 1);
    }

    // ---- Abandon from all non-terminal states ----

    #[test]
    fn abandon_from_all_non_terminal_states() {
        for phase in [
            InputLifecycleState::Accepted,
            InputLifecycleState::Queued,
            InputLifecycleState::Staged,
            InputLifecycleState::Applied,
            InputLifecycleState::AppliedPendingConsumption,
        ] {
            let mut auth = make_at_phase(phase);
            let t = auth.apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Destroyed,
            });
            assert!(
                t.is_ok(),
                "abandon should succeed from {phase:?}, got {t:?}"
            );
            assert!(auth.is_terminal());
        }
    }

    // ---- Abandon from terminal states rejected ----

    #[test]
    fn abandon_from_terminal_states_rejected() {
        for phase in [
            InputLifecycleState::Consumed,
            InputLifecycleState::Superseded,
            InputLifecycleState::Coalesced,
            InputLifecycleState::Abandoned,
        ] {
            let mut auth = make_at_phase(phase);
            let result = auth.apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Destroyed,
            });
            assert!(
                result.is_err(),
                "abandon should be rejected from terminal {phase:?}"
            );
        }
    }

    // ---- Effects emitted correctly ----

    #[test]
    fn consume_emits_notice_and_terminal_outcome() {
        let mut auth = make_authority();
        let run_id = RunId::new();
        auth.apply(InputLifecycleInput::QueueAccepted).unwrap();
        auth.apply(InputLifecycleInput::StageForRun {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkApplied {
            run_id: run_id.clone(),
        })
        .unwrap();
        auth.apply(InputLifecycleInput::MarkAppliedPendingConsumption {
            boundary_sequence: 1,
        })
        .unwrap();
        let t = auth.apply(InputLifecycleInput::Consume).unwrap();
        assert!(t.effects.iter().any(|e| matches!(
            e,
            InputLifecycleEffect::InputLifecycleNotice {
                new_state: InputLifecycleState::Consumed
            }
        )));
        assert!(t.effects.iter().any(|e| matches!(
            e,
            InputLifecycleEffect::RecordTerminalOutcome {
                outcome: InputTerminalOutcome::Consumed
            }
        )));
    }

    #[test]
    fn abandon_emits_notice_and_terminal_outcome() {
        let mut auth = make_authority();
        let t = auth
            .apply(InputLifecycleInput::Abandon {
                reason: InputAbandonReason::Cancelled,
            })
            .unwrap();
        assert!(t.effects.iter().any(|e| matches!(
            e,
            InputLifecycleEffect::InputLifecycleNotice {
                new_state: InputLifecycleState::Abandoned
            }
        )));
        assert!(t.effects.iter().any(|e| matches!(
            e,
            InputLifecycleEffect::RecordTerminalOutcome {
                outcome: InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled,
                },
            }
        )));
    }
}
