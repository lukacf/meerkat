//! Turn admission concurrency gate for `EphemeralSessionService`.
//!
//! `TurnAdmissionSlot` is a **shell concurrency gate**, not service lifecycle
//! authority. It serializes `start_turn` / interrupt / shutdown access on a
//! single session so at most one command is queued or running at a time and
//! shutdown drains cleanly through any in-flight command. Runtime-backed
//! service-visible lifecycle is projected by [`TurnServiceLifecycleAuthority`]
//! from the machine-owned [`meerkat_core::TurnStateSnapshot`]; the slot is only
//! a standalone compatibility fallback when no turn-state handle exists.

use std::fmt;

use meerkat_core::turn_execution_authority::TurnPhase;
use meerkat_core::{TurnStateSnapshot, TurnTerminalOutcome};

/// Admission phase tracked by the slot.
///
/// Ordering: `Idle → Admitted → Running → Completing → Idle` on the success
/// path; any phase may transition to `ShuttingDown` via `request_shutdown`
/// (with the shutdown commit deferred until `finalize` resolves any running
/// turn). `Admitted` and `Completing` have no DSL analogue — they are
/// service-level bookkeeping for a turn slot that has been reserved but not
/// yet run and for a run that has completed but not yet been finalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnAdmissionPhase {
    Idle,
    Admitted,
    Running,
    Completing,
    ShuttingDown,
}

/// Error returned when a mutator is invoked from an illegal phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnAdmissionError {
    pub(crate) from: TurnAdmissionPhase,
    pub(crate) op: &'static str,
}

impl fmt::Display for TurnAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "illegal turn-admission operation {:?} from phase {:?}",
            self.op, self.from
        )
    }
}

impl std::error::Error for TurnAdmissionError {}

/// Outcome of a successful `finalize` call — tells the caller whether the
/// session is continuing (back to `Idle`) or draining (`ShuttingDown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FinalizeOutcome {
    pub(crate) next_phase: TurnAdmissionPhase,
}

/// Copyable view of the shell gate. Callers pass snapshots into the lifecycle
/// projection authority instead of letting the slot drive service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnAdmissionSnapshot {
    pub(crate) phase: TurnAdmissionPhase,
}

/// Serialized turn-admission state for a single session.
#[derive(Debug, Clone)]
pub(crate) struct TurnAdmissionSlot {
    phase: TurnAdmissionPhase,
    interrupt_pending: bool,
    shutdown_pending: bool,
}

impl TurnAdmissionSlot {
    pub(crate) fn new() -> Self {
        Self {
            phase: TurnAdmissionPhase::Idle,
            interrupt_pending: false,
            shutdown_pending: false,
        }
    }

    pub(crate) fn phase(&self) -> TurnAdmissionPhase {
        self.phase
    }

    pub(crate) fn interrupt_pending(&self) -> bool {
        self.interrupt_pending
    }

    pub(crate) fn snapshot(&self) -> TurnAdmissionSnapshot {
        TurnAdmissionSnapshot { phase: self.phase }
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.phase,
            TurnAdmissionPhase::Admitted
                | TurnAdmissionPhase::Running
                | TurnAdmissionPhase::Completing
        )
    }

    /// Claim the turn slot before dispatching `StartTurn`. `Idle → Admitted`.
    pub(crate) fn claim(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Idle => {
                self.interrupt_pending = false;
                self.shutdown_pending = false;
                self.phase = TurnAdmissionPhase::Admitted;
                Ok(self.phase)
            }
            from => Err(TurnAdmissionError { from, op: "claim" }),
        }
    }

    /// Release an admitted slot without running the turn. `Admitted → Idle`.
    pub(crate) fn abort_claim(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Admitted => {
                self.interrupt_pending = false;
                self.shutdown_pending = false;
                self.phase = TurnAdmissionPhase::Idle;
                Ok(self.phase)
            }
            from => Err(TurnAdmissionError {
                from,
                op: "abort_claim",
            }),
        }
    }

    /// Mark the admitted slot as actively running. `Admitted → Running`.
    pub(crate) fn begin(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Admitted => {
                self.phase = TurnAdmissionPhase::Running;
                Ok(self.phase)
            }
            from => Err(TurnAdmissionError { from, op: "begin" }),
        }
    }

    /// Move a finished run into the finalization window. `Running → Completing`.
    pub(crate) fn resolve(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Running => {
                self.phase = TurnAdmissionPhase::Completing;
                Ok(self.phase)
            }
            from => Err(TurnAdmissionError {
                from,
                op: "resolve",
            }),
        }
    }

    /// Close the finalization window. `Completing → Idle` unless shutdown was
    /// requested mid-turn, in which case `Completing → ShuttingDown`.
    pub(crate) fn finalize(&mut self) -> Result<FinalizeOutcome, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Completing => {
                self.interrupt_pending = false;
                if self.shutdown_pending {
                    self.phase = TurnAdmissionPhase::ShuttingDown;
                } else {
                    self.shutdown_pending = false;
                    self.phase = TurnAdmissionPhase::Idle;
                }
                Ok(FinalizeOutcome {
                    next_phase: self.phase,
                })
            }
            from => Err(TurnAdmissionError {
                from,
                op: "finalize",
            }),
        }
    }

    /// Flag an interrupt request. Permitted once a turn slot has been admitted;
    /// returns `true` if the flag flipped from clear to set so the caller can
    /// wake any waiter.
    pub(crate) fn request_interrupt(&mut self) -> Result<bool, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Admitted | TurnAdmissionPhase::Running => {
                let already_pending = self.interrupt_pending;
                self.interrupt_pending = true;
                Ok(!already_pending)
            }
            from => Err(TurnAdmissionError {
                from,
                op: "request_interrupt",
            }),
        }
    }

    /// Flag a shutdown request. Transitions the slot to `ShuttingDown`
    /// immediately from `Idle` / `Admitted`; defers the transition until
    /// `finalize` runs if a turn is in flight (`Running` / `Completing`).
    pub(crate) fn request_shutdown(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Idle | TurnAdmissionPhase::Admitted => {
                self.interrupt_pending = false;
                self.shutdown_pending = true;
                self.phase = TurnAdmissionPhase::ShuttingDown;
                Ok(self.phase)
            }
            TurnAdmissionPhase::Running | TurnAdmissionPhase::Completing => {
                self.shutdown_pending = true;
                Ok(self.phase)
            }
            TurnAdmissionPhase::ShuttingDown => Ok(self.phase),
        }
    }
}

/// Service-facing activity projection for a live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnServiceLifecycleProjection {
    pub(crate) is_active: bool,
}

/// Owner for `EphemeralSessionService`'s public turn lifecycle projection.
///
/// Runtime-backed sessions derive activity from the turn-state handle snapshot
/// owned by MeerkatMachine. The shell admission slot participates only when no
/// machine handle exists, preserving standalone in-memory compatibility without
/// letting the slot become semantic authority for runtime sessions.
#[derive(Debug, Default)]
pub(crate) struct TurnServiceLifecycleAuthority;

impl TurnServiceLifecycleAuthority {
    pub(crate) fn project(
        admission: TurnAdmissionSnapshot,
        turn_state: Option<&TurnStateSnapshot>,
    ) -> TurnServiceLifecycleProjection {
        let is_active = match turn_state {
            Some(snapshot) => Self::machine_snapshot_is_active(snapshot),
            None => matches!(
                admission.phase,
                TurnAdmissionPhase::Admitted
                    | TurnAdmissionPhase::Running
                    | TurnAdmissionPhase::Completing
            ),
        };
        TurnServiceLifecycleProjection { is_active }
    }

    fn machine_snapshot_is_active(snapshot: &TurnStateSnapshot) -> bool {
        match snapshot.turn_phase {
            TurnPhase::Ready | TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled => {
                false
            }
            TurnPhase::ApplyingPrimitive
            | TurnPhase::CallingLlm
            | TurnPhase::WaitingForOps
            | TurnPhase::DrainingBoundary
            | TurnPhase::Extracting
            | TurnPhase::ErrorRecovery
            | TurnPhase::Cancelling => true,
        }
    }

    pub(crate) fn runtime_boundary_cancel_is_active(snapshot: &TurnStateSnapshot) -> bool {
        Self::machine_snapshot_is_active(snapshot)
            && !matches!(
                snapshot.terminal_outcome,
                Some(
                    TurnTerminalOutcome::Completed
                        | TurnTerminalOutcome::Failed
                        | TurnTerminalOutcome::Cancelled
                        | TurnTerminalOutcome::BudgetExhausted
                        | TurnTerminalOutcome::TimeBudgetExceeded
                        | TurnTerminalOutcome::StructuredOutputValidationFailed
                )
            )
    }
}

impl Default for TurnAdmissionSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn claim_reserves_slot() {
        let mut slot = TurnAdmissionSlot::new();
        let phase = slot.claim().expect("idle session should admit a turn");
        assert_eq!(phase, TurnAdmissionPhase::Admitted);
        assert!(slot.is_active());
    }

    #[test]
    fn interrupt_allowed_after_turn_admission() {
        let mut slot = TurnAdmissionSlot::new();
        let err = slot
            .request_interrupt()
            .expect_err("idle session cannot be interrupted");
        assert_eq!(err.from, TurnAdmissionPhase::Idle);

        slot.claim().unwrap();
        let admitted_woke = slot
            .request_interrupt()
            .expect("admitted session should accept interrupt");
        assert!(admitted_woke);
        assert!(slot.interrupt_pending());

        slot.begin().unwrap();
        let woke = slot
            .request_interrupt()
            .expect("running session should retain interrupt");
        assert!(!woke);
        assert_eq!(slot.phase(), TurnAdmissionPhase::Running);
        assert!(slot.interrupt_pending());
    }

    #[test]
    fn shutdown_gracefully_drains_running_turn() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().unwrap();
        slot.begin().unwrap();
        slot.request_shutdown().unwrap();
        assert_eq!(slot.phase(), TurnAdmissionPhase::Running);

        slot.resolve().unwrap();
        let outcome = slot
            .finalize()
            .expect("finalize should enter shutting down");
        assert_eq!(outcome.next_phase, TurnAdmissionPhase::ShuttingDown);
    }

    #[test]
    fn shutdown_cancels_admitted_before_run() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().unwrap();
        let phase = slot
            .request_shutdown()
            .expect("admitted turn should be shut down before run");
        assert_eq!(phase, TurnAdmissionPhase::ShuttingDown);
    }
}
