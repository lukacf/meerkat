//! Generated-authority module for the MobLifecycle machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all MobLifecycle state mutations flow through the machine authority.
//! Handwritten shell code calls [`MobLifecycleAuthority::apply`] and executes
//! returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/mob_lifecycle.rs`:
//!
//! - 5 states: Creating, Running, Stopped, Completed, Destroyed
//! - 9 inputs: Start, Stop, Resume, MarkCompleted, Destroy, StartRun,
//!   FinishRun, BeginCleanup, FinishCleanup
//! - 2 fields: active_run_count (u32), cleanup_pending (bool)
//! - 2 invariants: destroyed/completed have no active runs
//! - 8 transitions with guards (no_active_runs, has_active_runs)
//! - 2 effects: EmitLifecycleNotice, RequestCleanup

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::error::MobError;
use crate::runtime::MobState;

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the MobLifecycle machine.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`MobLifecycleAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // All variants are part of the machine schema; some not yet used by shell code.
pub(crate) enum MobLifecycleInput {
    Start,
    Stop,
    Resume,
    MarkCompleted,
    Destroy,
    StartRun,
    FinishRun,
    BeginCleanup,
    FinishCleanup,
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by MobLifecycle transitions.
///
/// Shell code receives these from [`MobLifecycleAuthority::apply`] and is
/// responsible for executing the side effects (e.g. emitting events, triggering
/// cleanup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MobLifecycleEffect {
    EmitLifecycleNotice,
    RequestCleanup,
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the MobLifecycle authority.
#[derive(Debug)]
#[allow(dead_code)] // Fields are part of the authority contract; inspected by tests and future shell code.
pub(crate) struct MobLifecycleTransition {
    /// The phase after the transition.
    pub next_phase: MobState,
    /// The active_run_count field after the transition.
    pub active_run_count: u32,
    /// The cleanup_pending field after the transition.
    pub cleanup_pending: bool,
    /// Effects to be executed by shell code.
    pub effects: Vec<MobLifecycleEffect>,
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

/// Canonical machine-owned fields for MobLifecycle.
#[derive(Debug, Clone)]
struct MobLifecycleFields {
    active_run_count: u32,
    cleanup_pending: bool,
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for MobLifecycle state mutation.
///
/// Only [`MobLifecycleAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for lifecycle state.
pub(crate) trait MobLifecycleMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(&mut self, input: MobLifecycleInput) -> Result<MobLifecycleTransition, MobError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for MobLifecycle state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table. Also maintains an `Arc<AtomicU8>` observable
/// cache so that `MobHandle` can read the current phase lock-free.
#[derive(Clone)]
pub(crate) struct MobLifecycleAuthority {
    /// Canonical phase.
    phase: MobState,
    /// Canonical machine-owned fields.
    fields: MobLifecycleFields,
    /// Observable cache for lock-free reads from MobHandle.
    /// Set by the authority after each transition — never written elsewhere.
    observable: Arc<AtomicU8>,
}

impl sealed::Sealed for MobLifecycleAuthority {}

impl MobLifecycleAuthority {
    /// Create an authority initialized to a specific phase.
    ///
    /// Used during mob creation (phase = Running after builder emits
    /// MobCreated) and during resume (phase recovered from events).
    pub(crate) fn with_phase(observable: Arc<AtomicU8>, phase: MobState) -> Self {
        observable.store(phase as u8, Ordering::Release);
        Self {
            phase,
            fields: MobLifecycleFields {
                active_run_count: 0,
                cleanup_pending: false,
            },
            observable,
        }
    }

    /// Current phase (read from canonical state).
    pub(crate) fn phase(&self) -> MobState {
        self.phase
    }

    /// Current active_run_count from canonical state.
    #[cfg(test)]
    pub(crate) fn active_run_count(&self) -> u32 {
        self.fields.active_run_count
    }

    /// Check if a transition is legal without applying it.
    ///
    /// Used by shell code for pre-checks (e.g. Destroy/Reset eligibility).
    pub(crate) fn can_accept(&self, input: MobLifecycleInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Require that the authority is in one of the given phases.
    ///
    /// Returns `MobError::InvalidTransition` if not. The `hint_to` parameter
    /// provides a target state for the error message (not a real transition).
    pub(crate) fn require_phase(
        &self,
        allowed: &[MobState],
        hint_to: MobState,
    ) -> Result<(), MobError> {
        if allowed.contains(&self.phase) {
            Ok(())
        } else {
            Err(MobError::InvalidTransition {
                from: self.phase,
                to: hint_to,
            })
        }
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: MobLifecycleInput,
    ) -> Result<(MobState, MobLifecycleFields, Vec<MobLifecycleEffect>), MobError> {
        use MobLifecycleInput::{
            BeginCleanup, Destroy, FinishCleanup, FinishRun, MarkCompleted, Resume, Start,
            StartRun, Stop,
        };
        use MobState::{Completed, Creating, Destroyed, Running, Stopped};

        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
            // Start: Creating|Stopped -> Running
            (Creating | Stopped, Start) => Running,

            // Stop: Running -> Stopped
            (Running, Stop) => Stopped,

            // Resume: Stopped -> Running
            (Stopped, Resume) => Running,

            // MarkCompleted: Running|Stopped -> Completed (guard: no_active_runs)
            (Running | Stopped, MarkCompleted) => {
                if fields.active_run_count != 0 {
                    return Err(MobError::InvalidTransition {
                        from: phase,
                        to: Completed,
                    });
                }
                Completed
            }

            // Destroy: Creating|Running|Stopped|Completed -> Destroyed
            // Updates: active_run_count = 0, cleanup_pending = false
            // Emits: EmitLifecycleNotice
            (Creating | Running | Stopped | Completed, Destroy) => {
                fields.active_run_count = 0;
                fields.cleanup_pending = false;
                effects.push(MobLifecycleEffect::EmitLifecycleNotice);
                Destroyed
            }

            // StartRun: Running -> Running
            // Updates: active_run_count += 1
            (Running, StartRun) => {
                fields.active_run_count = fields.active_run_count.saturating_add(1);
                Running
            }

            // FinishRun: Running|Stopped -> Running (guard: has_active_runs)
            // Updates: active_run_count -= 1
            (Running | Stopped, FinishRun) => {
                if fields.active_run_count == 0 {
                    return Err(MobError::InvalidTransition {
                        from: phase,
                        to: Running,
                    });
                }
                fields.active_run_count -= 1;
                // FinishRun keeps the same phase per schema (to: Running),
                // but from Stopped it should stay in the current phase.
                // The schema says `to: "Running"` for FinishRun, so we follow that.
                Running
            }

            // BeginCleanup: Stopped|Completed -> Stopped
            // Updates: cleanup_pending = true
            // Emits: RequestCleanup
            (Stopped | Completed, BeginCleanup) => {
                fields.cleanup_pending = true;
                effects.push(MobLifecycleEffect::RequestCleanup);
                Stopped
            }

            // FinishCleanup: Stopped|Completed -> Stopped
            // Updates: cleanup_pending = false
            (Stopped | Completed, FinishCleanup) => {
                fields.cleanup_pending = false;
                Stopped
            }

            // All other combinations are illegal.
            _ => {
                let target = match input {
                    Start | Resume | StartRun => Running,
                    Stop | BeginCleanup | FinishCleanup | FinishRun => Stopped,
                    MarkCompleted => Completed,
                    Destroy => Destroyed,
                };
                return Err(MobError::InvalidTransition {
                    from: phase,
                    to: target,
                });
            }
        };

        Ok((next_phase, fields, effects))
    }
}

impl MobLifecycleMutator for MobLifecycleAuthority {
    fn apply(&mut self, input: MobLifecycleInput) -> Result<MobLifecycleTransition, MobError> {
        let (next_phase, next_fields, effects) = self.evaluate(input)?;

        // Commit: update canonical state and observable cache.
        self.phase = next_phase;
        self.fields = next_fields.clone();
        self.observable.store(next_phase as u8, Ordering::Release);

        Ok(MobLifecycleTransition {
            next_phase,
            active_run_count: next_fields.active_run_count,
            cleanup_pending: next_fields.cleanup_pending,
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_authority(phase: MobState) -> MobLifecycleAuthority {
        MobLifecycleAuthority::with_phase(Arc::new(AtomicU8::new(phase as u8)), phase)
    }

    #[test]
    fn start_from_creating_transitions_to_running() {
        let mut auth = make_authority(MobState::Creating);
        let result = auth.apply(MobLifecycleInput::Start);
        let transition = result.expect("start from creating should succeed");
        assert_eq!(transition.next_phase, MobState::Running);
        assert_eq!(auth.phase(), MobState::Running);
    }

    #[test]
    fn start_from_stopped_transitions_to_running() {
        let mut auth = make_authority(MobState::Stopped);
        let t = auth
            .apply(MobLifecycleInput::Start)
            .expect("start from stopped");
        assert_eq!(t.next_phase, MobState::Running);
    }

    #[test]
    fn stop_from_running_transitions_to_stopped() {
        let mut auth = make_authority(MobState::Running);
        let t = auth
            .apply(MobLifecycleInput::Stop)
            .expect("stop should succeed");
        assert_eq!(t.next_phase, MobState::Stopped);
    }

    #[test]
    fn resume_from_stopped_transitions_to_running() {
        let mut auth = make_authority(MobState::Stopped);
        let t = auth
            .apply(MobLifecycleInput::Resume)
            .expect("resume should succeed");
        assert_eq!(t.next_phase, MobState::Running);
    }

    #[test]
    fn start_run_increments_active_run_count() {
        let mut auth = make_authority(MobState::Running);
        let t1 = auth
            .apply(MobLifecycleInput::StartRun)
            .expect("start run 1");
        assert_eq!(t1.active_run_count, 1);
        let t2 = auth
            .apply(MobLifecycleInput::StartRun)
            .expect("start run 2");
        assert_eq!(t2.active_run_count, 2);
        assert_eq!(auth.active_run_count(), 2);
    }

    #[test]
    fn finish_run_decrements_active_run_count() {
        let mut auth = make_authority(MobState::Running);
        auth.apply(MobLifecycleInput::StartRun).expect("start run");
        auth.apply(MobLifecycleInput::StartRun)
            .expect("start run 2");
        let t = auth
            .apply(MobLifecycleInput::FinishRun)
            .expect("finish run");
        assert_eq!(t.active_run_count, 1);
    }

    #[test]
    fn finish_run_rejects_zero_active_runs() {
        let mut auth = make_authority(MobState::Running);
        let result = auth.apply(MobLifecycleInput::FinishRun);
        assert!(
            result.is_err(),
            "should reject FinishRun with zero active runs"
        );
    }

    #[test]
    fn mark_completed_requires_no_active_runs() {
        let mut auth = make_authority(MobState::Running);
        auth.apply(MobLifecycleInput::StartRun).expect("start run");
        let result = auth.apply(MobLifecycleInput::MarkCompleted);
        assert!(
            result.is_err(),
            "should reject MarkCompleted with active runs"
        );
    }

    #[test]
    fn mark_completed_succeeds_with_zero_active_runs() {
        let mut auth = make_authority(MobState::Running);
        let t = auth
            .apply(MobLifecycleInput::MarkCompleted)
            .expect("mark completed");
        assert_eq!(t.next_phase, MobState::Completed);
    }

    #[test]
    fn destroy_emits_lifecycle_notice() {
        let mut auth = make_authority(MobState::Running);
        let t = auth.apply(MobLifecycleInput::Destroy).expect("destroy");
        assert_eq!(t.next_phase, MobState::Destroyed);
        assert!(t.effects.contains(&MobLifecycleEffect::EmitLifecycleNotice));
    }

    #[test]
    fn destroy_clears_fields() {
        let mut auth = make_authority(MobState::Running);
        auth.apply(MobLifecycleInput::StartRun).expect("start run");
        let t = auth.apply(MobLifecycleInput::Destroy).expect("destroy");
        assert_eq!(t.active_run_count, 0);
        assert!(!t.cleanup_pending);
    }

    #[test]
    fn begin_cleanup_emits_request_cleanup() {
        let mut auth = make_authority(MobState::Stopped);
        let t = auth
            .apply(MobLifecycleInput::BeginCleanup)
            .expect("begin cleanup");
        assert!(t.cleanup_pending);
        assert!(t.effects.contains(&MobLifecycleEffect::RequestCleanup));
    }

    #[test]
    fn finish_cleanup_clears_cleanup_pending() {
        let mut auth = make_authority(MobState::Stopped);
        auth.apply(MobLifecycleInput::BeginCleanup)
            .expect("begin cleanup");
        let t = auth
            .apply(MobLifecycleInput::FinishCleanup)
            .expect("finish cleanup");
        assert!(!t.cleanup_pending);
    }

    #[test]
    fn observable_cache_is_updated_on_transition() {
        let observable = Arc::new(AtomicU8::new(0));
        let mut auth = MobLifecycleAuthority::with_phase(observable.clone(), MobState::Creating);
        assert_eq!(observable.load(Ordering::Acquire), MobState::Creating as u8);
        auth.apply(MobLifecycleInput::Start).expect("start");
        assert_eq!(observable.load(Ordering::Acquire), MobState::Running as u8);
    }

    #[test]
    fn reject_invalid_transition() {
        let mut auth = make_authority(MobState::Completed);
        let result = auth.apply(MobLifecycleInput::Start);
        assert!(result.is_err());
        // Phase should not change on failure.
        assert_eq!(auth.phase(), MobState::Completed);
    }

    #[test]
    fn require_phase_accepts_allowed() {
        let auth = make_authority(MobState::Running);
        assert!(
            auth.require_phase(&[MobState::Running, MobState::Creating], MobState::Running)
                .is_ok()
        );
    }

    #[test]
    fn require_phase_rejects_disallowed() {
        let auth = make_authority(MobState::Stopped);
        let result = auth.require_phase(&[MobState::Running], MobState::Running);
        assert!(matches!(result, Err(MobError::InvalidTransition { .. })));
    }

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = make_authority(MobState::Running);
        assert!(auth.can_accept(MobLifecycleInput::Stop));
        assert!(!auth.can_accept(MobLifecycleInput::Resume));
        // Phase is still Running — no mutation.
        assert_eq!(auth.phase(), MobState::Running);
    }

    #[test]
    fn destroy_from_all_valid_phases() {
        for phase in [
            MobState::Creating,
            MobState::Running,
            MobState::Stopped,
            MobState::Completed,
        ] {
            let mut auth = make_authority(phase);
            let t = auth
                .apply(MobLifecycleInput::Destroy)
                .unwrap_or_else(|_| panic!("destroy should work from {phase}"));
            assert_eq!(t.next_phase, MobState::Destroyed);
        }
    }

    #[test]
    fn destroy_from_destroyed_is_rejected() {
        let mut auth = make_authority(MobState::Destroyed);
        assert!(auth.apply(MobLifecycleInput::Destroy).is_err());
    }

    #[test]
    fn stop_from_non_running_is_rejected() {
        for phase in [MobState::Creating, MobState::Completed, MobState::Destroyed] {
            let mut auth = make_authority(phase);
            assert!(
                auth.apply(MobLifecycleInput::Stop).is_err(),
                "stop should be rejected from {phase}"
            );
        }
    }

    #[test]
    fn start_run_from_non_running_is_rejected() {
        let mut auth = make_authority(MobState::Stopped);
        assert!(auth.apply(MobLifecycleInput::StartRun).is_err());
    }
}
