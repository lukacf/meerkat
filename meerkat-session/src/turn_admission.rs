//! Turn admission concurrency slot for `EphemeralSessionService`.
//!
//! This is a **shell concurrency guard**, not a state machine. It serializes
//! `start_turn` / interrupt / shutdown access on a single session across the
//! service API surface so that (a) at most one turn is admitted or running at
//! any time and (b) shutdown drains cleanly through any in-flight or claimed
//! turn. It lives entirely inside `EphemeralSessionService`; it is not driven
//! by, and does not participate in, the MeerkatMachine DSL.
//!
//! Plain struct with named mutators — no `apply(Input) -> Transition` shape,
//! no authority trait, no transition table. Analogous in spirit to a
//! specialized `Semaphore`.

use std::fmt;

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

    /// Flag an interrupt request. Only permitted while a turn is actively
    /// running; returns `true` if the flag flipped from clear to set so the
    /// caller can wake any waiter.
    pub(crate) fn request_interrupt(&mut self) -> Result<bool, TurnAdmissionError> {
        match self.phase {
            TurnAdmissionPhase::Running => {
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
    fn interrupt_only_allowed_while_running() {
        let mut slot = TurnAdmissionSlot::new();
        let err = slot
            .request_interrupt()
            .expect_err("idle session cannot be interrupted");
        assert_eq!(err.from, TurnAdmissionPhase::Idle);

        slot.claim().unwrap();
        slot.begin().unwrap();
        let woke = slot
            .request_interrupt()
            .expect("running session should accept interrupt");
        assert!(woke);
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
