//! Runtime impl of [`meerkat_core::handles::SessionContextHandle`] (W2-E).
//!
//! Routes every canonical session-truth mutation into the session's
//! MeerkatMachine DSL via the `AdvanceSessionContext` input. The
//! transition is monotonically guarded at the DSL layer, so callers fire
//! unconditionally post-mutation and the DSL drops duplicate or
//! out-of-order ticks.
//!
//! Every successful transition emits `SessionContextAdvanced`; the
//! handle scans effects and dispatches to the installed
//! [`meerkat_core::handles::SessionContextAdvancedObserver`]. The
//! realtime projection consumer uses this observer to drive its typed
//! `ProjectionFreshness` state — replacing the hand-wired
//! `projection_refresh_rx` polling channel + `projection_refresh_dirty`
//! flag.

use std::sync::{Arc, RwLock, Weak};

use meerkat_core::handles::{
    DslTransitionError, SessionContextAdvancedObserver, SessionContextHandle,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`SessionContextHandle`] impl.
///
/// Mirrors the pattern used by [`super::RuntimePeerInteractionHandle`]:
/// the observer is held as a `Weak` so this handle does not keep the
/// realtime projection consumer alive past its socket's lifetime.
pub struct RuntimeSessionContextHandle {
    dsl: Arc<HandleDslAuthority>,
    observer: RwLock<Option<Weak<dyn SessionContextAdvancedObserver>>>,
}

impl std::fmt::Debug for RuntimeSessionContextHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let observer_tag = self
            .observer
            .read()
            .ok()
            .as_deref()
            .and_then(|o| o.as_ref().map(|_| "<observer>"));
        f.debug_struct("RuntimeSessionContextHandle")
            .field("dsl", &self.dsl)
            .field("observer", &observer_tag)
            .finish()
    }
}

impl RuntimeSessionContextHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            observer: RwLock::new(None),
        }
    }

    /// Construct a handle backed by an ephemeral DSL authority (tests /
    /// legacy recovery paths).
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl SessionContextHandle for RuntimeSessionContextHandle {
    fn context_advanced(&self, updated_at_ms: u64) -> Result<bool, DslTransitionError> {
        let effects = match self.dsl.apply_input_with_effects(
            mm_dsl::MeerkatMachineInput::AdvanceSessionContext { updated_at_ms },
            "SessionContextHandle::context_advanced",
        ) {
            Ok(effects) => effects,
            // The monotonic guard surfaces as a typed `GuardRejected` —
            // treat as `Ok(false)` so callers can fire unconditionally
            // without tracking their own watermark. Any other rejection
            // (e.g., `NoMatchingTransition` from a mis-phased call) is a
            // real error and propagates.
            Err(err) if err.is_guard_rejected() => return Ok(false),
            Err(err) => return Err(err),
        };

        let observer_opt = self
            .observer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(observer) = observer_opt {
            for effect in effects {
                if let mm_dsl::MeerkatMachineEffect::SessionContextAdvanced {
                    updated_at_ms: emitted,
                } = effect
                {
                    observer.on_session_context_advanced(emitted);
                }
            }
        }
        Ok(true)
    }

    fn current_watermark_ms(&self) -> u64 {
        self.dsl.snapshot_state().last_session_context_updated_at_ms
    }

    fn install_observer(&self, observer: Arc<dyn SessionContextAdvancedObserver>) {
        *self
            .observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&observer));
    }

    fn install_observer_with_baseline(
        &self,
        observer: Arc<dyn SessionContextAdvancedObserver>,
    ) -> u64 {
        // Atomic critical section: hold the DSL authority lock while
        // installing the observer. Any `context_advanced` call that runs
        // concurrently either (a) completes before this function acquires
        // the DSL lock — its advance is recorded in the returned baseline
        // and its observer-notify is dropped because no observer is
        // installed yet, or (b) blocks until we release the DSL lock —
        // by then the observer is installed, so the next `context_advanced`
        // notify lands. In both cases the consumer's baseline and the
        // observer's effect stream agree on the frontier.
        //
        // Locking order matches `apply_input_with_effects` (DSL first,
        // observer second) so this critical section cannot deadlock with
        // a concurrent transition.
        self.dsl.with_state_lock(|state| {
            *self
                .observer
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::downgrade(&observer));
            state.last_session_context_updated_at_ms
        })
    }
}
