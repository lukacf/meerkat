//! Runtime impl of [`meerkat_core::handles::CommsDrainHandle`].

use std::sync::Arc;

use meerkat_core::handles::{CommsDrainHandle, DslTransitionError};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`CommsDrainHandle`] impl.
///
/// Routes every trait method to the corresponding DSL input / signal on a
/// dedicated per-session MeerkatMachine DSL authority.
#[derive(Debug)]
pub struct RuntimeCommsDrainHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeCommsDrainHandle {
    /// Construct a handle backed by a fresh DSL authority.
    pub fn new() -> Self {
        Self {
            dsl: Arc::new(HandleDslAuthority::new()),
        }
    }
}

impl Default for RuntimeCommsDrainHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl CommsDrainHandle for RuntimeCommsDrainHandle {
    fn ensure_drain_running(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::EnsureDrainRunning,
            "CommsDrainHandle::ensure_drain_running",
        )
    }

    fn spawn_drain(&self, mode: &str) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SpawnDrain {
                mode: mode.to_string(),
            },
            "CommsDrainHandle::spawn_drain",
        )
    }

    fn stop_drain(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::StopDrain,
            "CommsDrainHandle::stop_drain",
        )
    }

    fn drain_exited_clean(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::DrainExitedClean,
            "CommsDrainHandle::drain_exited_clean",
        )
    }

    fn drain_exited_respawnable(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::DrainExitedRespawnable,
            "CommsDrainHandle::drain_exited_respawnable",
        )
    }

    fn notify_drain_exited(&self, reason: &str) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::NotifyDrainExited {
                reason: reason.to_string(),
            },
            "CommsDrainHandle::notify_drain_exited",
        )
    }
}
