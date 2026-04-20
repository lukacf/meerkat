//! Runtime impl of [`meerkat_core::handles::CommsDrainHandle`].

use std::sync::Arc;

use meerkat_core::handles::{CommsDrainHandle, DrainExitReason, DrainMode, DslTransitionError};

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
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    ///
    /// See [`RuntimeTurnStateHandle::ephemeral`].
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl CommsDrainHandle for RuntimeCommsDrainHandle {
    fn ensure_drain_running(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::EnsureDrainRunning,
            "CommsDrainHandle::ensure_drain_running",
        )
    }

    fn spawn_drain(&self, mode: DrainMode) -> Result<(), DslTransitionError> {
        let mode = match mode {
            DrainMode::Timed => mm_dsl::DrainMode::Timed,
            DrainMode::AttachedSession => mm_dsl::DrainMode::AttachedSession,
            DrainMode::PersistentHost => mm_dsl::DrainMode::PersistentHost,
        };
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SpawnDrain { mode },
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

    fn notify_drain_exited(&self, reason: DrainExitReason) -> Result<(), DslTransitionError> {
        // The DSL `reason` slot is still a literal-string today; it will retype
        // in lockstep with the DSL input's slot. The handle contract already
        // carries the typed enum so callers cannot misspell a discriminant.
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::NotifyDrainExited {
                reason: reason.as_str().to_owned(),
            },
            "CommsDrainHandle::notify_drain_exited",
        )
    }
}
