//! Runtime-side impls of the cross-crate DSL handle traits defined in
//! `meerkat_core::handles`.
//!
//! Each handle holds a dedicated per-session MeerkatMachine DSL authority
//! behind a `std::sync::Mutex`. Trait methods are sync (matching the trait
//! contract in meerkat-core), so tokio-async access to the session's main DSL
//! is not suitable — instead each handle owns its own DSL substate, following
//! the [`crate::ops_lifecycle::RuntimeOpsLifecycleRegistry`] precedent.
//!
//! Phase 5F/0 is a pure addition commit: these handles are constructed by
//! [`crate::meerkat_machine::MeerkatMachine::prepare_bindings`] and populated
//! on [`meerkat_core::SessionRuntimeBindings`], but no existing callsites
//! dispatch through them yet. Phases 5F/1-5 flip callsites to the new handles.

use std::sync::Mutex;

use crate::meerkat_machine::dsl as mm_dsl;
use meerkat_core::handles::DslTransitionError;

mod comms_drain;
mod external_tool_surface;
mod peer_comms;
mod session_admission;
mod turn_state;

pub use comms_drain::RuntimeCommsDrainHandle;
pub use external_tool_surface::RuntimeExternalToolSurfaceHandle;
pub use peer_comms::RuntimePeerCommsHandle;
pub use session_admission::RuntimeSessionAdmissionHandle;
pub use turn_state::RuntimeTurnStateHandle;

/// Shared wrapper around the DSL authority used by all 5 handle impls.
///
/// Sync `Mutex` because trait methods are sync; per-handle ownership means
/// no lock contention with the session's main (tokio-guarded) DSL state.
pub(crate) struct HandleDslAuthority {
    inner: Mutex<mm_dsl::MeerkatMachineAuthority>,
}

impl HandleDslAuthority {
    /// Construct a fresh DSL authority at the initial `Initializing` phase.
    pub(crate) fn new() -> Self {
        let state = mm_dsl::MeerkatMachineState::default();
        Self {
            inner: Mutex::new(mm_dsl::MeerkatMachineAuthority::from_state(state)),
        }
    }

    /// Apply a DSL input under the handle's local mutex.
    pub(crate) fn apply_input(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mm_dsl::MeerkatMachineMutator::apply(&mut *guard, input)
            .map(|_| ())
            .map_err(|err| DslTransitionError::new(context, format!("{err}")))
    }

    /// Apply a DSL signal under the handle's local mutex.
    pub(crate) fn apply_signal(
        &self,
        signal: mm_dsl::MeerkatMachineSignal,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .apply_signal(signal)
            .map(|_| ())
            .map_err(|err| DslTransitionError::new(context, format!("{err}")))
    }
}

impl std::fmt::Debug for HandleDslAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandleDslAuthority").finish_non_exhaustive()
    }
}
