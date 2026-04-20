//! Runtime-side impls of the cross-crate DSL handle traits defined in
//! `meerkat_core::handles`.
//!
//! Each handle holds an `Arc<std::sync::Mutex<mm_dsl::MeerkatMachineAuthority>>`
//! that points at the **session's real DSL authority** — the same instance
//! stored on [`crate::meerkat_machine::RuntimeSessionEntry::dsl_authority`].
//! All 5 handles for a given session share the same `Arc`, so transitions
//! fired through any handle land on the session's canonical DSL state.
//!
//! Sync `std::sync::Mutex` is used (not tokio's async lock) because the
//! [`meerkat_core::handles`] trait methods are sync. Shell code that already
//! mutates the session DSL authority under the `sessions` tokio lock takes the
//! same inner sync lock via [`HandleDslAuthority::apply_input`] /
//! [`HandleDslAuthority::apply_signal`]; locks are held briefly across a
//! single DSL transition, so contention is not a concern.
//!
//! Phase 5F/0 is a pure addition commit: these handles are constructed by
//! [`crate::meerkat_machine::MeerkatMachine::prepare_bindings`] and populated
//! on [`meerkat_core::SessionRuntimeBindings`], but no existing callsites
//! dispatch through them yet. Phases 5F/1-5 flip callsites to the new handles.

use std::sync::{Arc, Mutex};

use crate::meerkat_machine::dsl as mm_dsl;
use meerkat_core::handles::DslTransitionError;

mod auth_lease;
mod comms_drain;
mod external_tool_surface;
mod mcp_server_lifecycle;
mod peer_comms;
mod peer_interaction;
mod session_admission;
mod session_claim;
mod session_context;
mod turn_state;

pub use auth_lease::RuntimeAuthLeaseHandle;
pub use comms_drain::RuntimeCommsDrainHandle;
pub use external_tool_surface::RuntimeExternalToolSurfaceHandle;
pub use mcp_server_lifecycle::RuntimeMcpServerLifecycleHandle;
pub use peer_comms::RuntimePeerCommsHandle;
pub use peer_interaction::RuntimePeerInteractionHandle;
pub use session_admission::RuntimeSessionAdmissionHandle;
pub use session_claim::RuntimeSessionClaimRegistry;
pub use session_context::RuntimeSessionContextHandle;
pub use turn_state::RuntimeTurnStateHandle;

/// Shared handle over a session's real `MeerkatMachineAuthority`.
///
/// Constructed by [`crate::meerkat_machine::MeerkatMachine::prepare_bindings`]
/// from the session's [`crate::meerkat_machine::RuntimeSessionEntry::dsl_authority`]
/// `Arc`; cloned into each of the 5 handle impls so all routes mutate the same
/// underlying authority.
///
/// A standalone ephemeral constructor ([`HandleDslAuthority::ephemeral`]) is
/// also provided for legacy code paths (recovery fallback, test sites) that
/// do not yet have a session-owned DSL authority to share. Ephemeral
/// authorities do not synchronize with any other state; transitions land on a
/// private initial DSL state only.
pub struct HandleDslAuthority {
    inner: Arc<Mutex<mm_dsl::MeerkatMachineAuthority>>,
}

impl HandleDslAuthority {
    /// Wrap an existing shared DSL authority. The returned handle and the
    /// caller's `Arc` both point at the same underlying authority instance.
    pub fn from_shared(inner: Arc<Mutex<mm_dsl::MeerkatMachineAuthority>>) -> Self {
        Self { inner }
    }

    /// Construct a handle with its own ephemeral DSL authority at the initial
    /// state.
    ///
    /// Legacy callers without access to a session-owned authority use this for
    /// compile-time correctness of `SessionRuntimeBindings`. Transitions fired
    /// through a handle backed by this authority are not visible to any other
    /// session state.
    pub fn ephemeral() -> Self {
        let state = mm_dsl::MeerkatMachineState::default();
        Self {
            inner: Arc::new(Mutex::new(mm_dsl::MeerkatMachineAuthority::from_state(
                state,
            ))),
        }
    }

    /// Apply a DSL input under the shared authority's mutex.
    pub fn apply_input(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        self.apply_input_with_effects(input, context).map(|_| ())
    }

    /// Apply a DSL input and return the emitted effects.
    ///
    /// Handles that need to react to effect emission (e.g.,
    /// [`crate::handles::RuntimePeerInteractionHandle`] consuming
    /// `PeerInteractionCleanup` to drop shell-side channel projections)
    /// use this variant so the effect is observed under the same lock as
    /// the state update — the "terminal transition → effect → cleanup"
    /// chain is causal, not lexically adjacent.
    pub fn apply_input_with_effects(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<Vec<mm_dsl::MeerkatMachineEffect>, DslTransitionError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mm_dsl::MeerkatMachineMutator::apply(&mut *guard, input)
            .map(|transition| transition.effects)
            .map_err(|err| DslTransitionError::new(context, format!("{err}")))
    }

    /// Apply a DSL signal under the shared authority's mutex.
    pub fn apply_signal(
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

    /// Clone the current DSL state under the shared authority's mutex.
    pub fn snapshot_state(&self) -> mm_dsl::MeerkatMachineState {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.state.clone()
    }

    /// Run `body` under the shared authority's mutex. The closure observes
    /// the DSL state atomically with any side effects it performs on the
    /// handle's external state (e.g. installing an observer before any
    /// further `apply_input_with_effects` can run). Locking order must
    /// match the order used by `apply_input_with_effects` (DSL first) so
    /// callers can safely acquire additional locks inside the closure.
    pub fn with_state_lock<R>(&self, body: impl FnOnce(&mm_dsl::MeerkatMachineState) -> R) -> R {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        body(&guard.state)
    }
}

impl std::fmt::Debug for HandleDslAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandleDslAuthority").finish_non_exhaustive()
    }
}
