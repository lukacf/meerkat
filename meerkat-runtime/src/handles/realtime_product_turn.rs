//! Runtime impl of [`meerkat_core::handles::RealtimeProductTurnHandle`]
//! (U9 / dogma #4).
//!
//! Routes every realtime product-turn lifecycle observation into the
//! session's MeerkatMachine DSL. Replaces the shell-local boolean triple
//! (`product_turn_in_flight`, `product_turn_committed`,
//! `product_output_started`) + helper-local event matching that used to
//! live in `meerkat-rpc::realtime_ws`.

use std::sync::Arc;

use meerkat_core::handles::{
    DslTransitionError, RealtimeProductTurnHandle, RealtimeProductTurnPhase,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`RealtimeProductTurnHandle`] impl.
pub struct RuntimeRealtimeProductTurnHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl std::fmt::Debug for RuntimeRealtimeProductTurnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeRealtimeProductTurnHandle")
            .field("dsl", &self.dsl)
            .finish()
    }
}

impl RuntimeRealtimeProductTurnHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority (tests /
    /// legacy recovery paths).
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }

    /// Apply the input; map typed guard rejection (all five product-turn
    /// transitions are idempotent via guards) to `Ok(false)` while
    /// propagating any other transition error. Classification routes
    /// through the typed [`meerkat_core::handles::DslRejectionKind`] so
    /// no substring matching on rendered messages is required.
    fn apply_idempotent(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<bool, DslTransitionError> {
        match self.dsl.apply_input(input, context) {
            Ok(()) => Ok(true),
            Err(err) if err.is_guard_rejected() => Ok(false),
            Err(err) => Err(err),
        }
    }
}

fn map_phase(raw: mm_dsl::RealtimeProductTurnPhase) -> RealtimeProductTurnPhase {
    match raw {
        mm_dsl::RealtimeProductTurnPhase::Idle => RealtimeProductTurnPhase::Idle,
        mm_dsl::RealtimeProductTurnPhase::AwaitingProgress => {
            RealtimeProductTurnPhase::AwaitingProgress
        }
        mm_dsl::RealtimeProductTurnPhase::Committed => RealtimeProductTurnPhase::Committed,
        mm_dsl::RealtimeProductTurnPhase::OutputStarted => RealtimeProductTurnPhase::OutputStarted,
        mm_dsl::RealtimeProductTurnPhase::Preemptible => RealtimeProductTurnPhase::Preemptible,
    }
}

impl RealtimeProductTurnHandle for RuntimeRealtimeProductTurnHandle {
    fn turn_in_flight(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ProductTurnInFlight,
            "RealtimeProductTurnHandle::turn_in_flight",
        )
    }

    fn turn_committed(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ProductTurnCommitted,
            "RealtimeProductTurnHandle::turn_committed",
        )
    }

    fn output_started(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ProductOutputStarted,
            "RealtimeProductTurnHandle::output_started",
        )
    }

    fn turn_interrupted(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ProductTurnInterrupted,
            "RealtimeProductTurnHandle::turn_interrupted",
        )
    }

    fn turn_terminal(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ProductTurnTerminal,
            "RealtimeProductTurnHandle::turn_terminal",
        )
    }

    fn current_phase(&self) -> RealtimeProductTurnPhase {
        map_phase(self.dsl.snapshot_state().realtime_product_turn_phase)
    }
}
