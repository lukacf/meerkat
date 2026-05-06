//! Runtime impl of [`meerkat_core::handles::RealtimeProductTurnHandle`]
//! (U9 / dogma #4, dogma round 2 U-C / dogma #1, #3, #13, #18, #20).
//!
//! Routes every realtime product-turn lifecycle observation into the
//! session's MeerkatMachine DSL. Replaces the shell-local boolean triple
//! (`product_turn_in_flight`, `product_turn_committed`,
//! `product_output_started`) + helper-local event matching that used to
//! live in `meerkat-rpc::realtime_ws`.
//!
//! Dogma round 2 (U-C) additionally routes the realtime projection
//! freshness (`RealtimeProjectionFreshness` state machine) and the clean-
//! close reconnect policy (`RealtimeReconnectPolicy`) through this same
//! handle — both previously lived as shell-local typed state on the
//! websocket task. The handle holds a `Weak` ref to the freshness
//! observer so it does not keep the socket's dispatcher alive past its
//! natural lifetime.

use std::sync::{Arc, RwLock, Weak};

use meerkat_core::handles::{
    DslTransitionError, RealtimeProductTurnHandle, RealtimeProductTurnPhase,
    RealtimeProjectionFreshness, RealtimeProjectionFreshnessObserver, RealtimeReconnectPolicy,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`RealtimeProductTurnHandle`] impl.
pub struct RuntimeRealtimeProductTurnHandle {
    dsl: Arc<HandleDslAuthority>,
    freshness_observer: RwLock<Option<Weak<dyn RealtimeProjectionFreshnessObserver>>>,
}

impl std::fmt::Debug for RuntimeRealtimeProductTurnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let observer_tag = self
            .freshness_observer
            .read()
            .ok()
            .as_deref()
            .and_then(|o| o.as_ref().map(|_| "<observer>"));
        f.debug_struct("RuntimeRealtimeProductTurnHandle")
            .field("dsl", &self.dsl)
            .field("freshness_observer", &observer_tag)
            .finish()
    }
}

impl RuntimeRealtimeProductTurnHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            freshness_observer: RwLock::new(None),
        }
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
        // intra-machine: no route; dispatcher not applicable
        // (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        match self.dsl.apply_input(input, context) {
            Ok(()) => Ok(true),
            Err(err) if err.is_guard_rejected() => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Apply an input that may emit `RealtimeProjectionFreshnessChanged`
    /// effects, sampling the installed freshness observer under the
    /// same DSL lock that committed the transition and dispatching the
    /// observer callback post-lock.
    ///
    /// Same race-closing pattern as `session_context.rs`: the sample
    /// inside the DSL critical section ensures a concurrent
    /// `install_projection_freshness_observer` is totally ordered vs
    /// this transition, so a just-installed observer cannot receive a
    /// fire whose state it already reflects in its install snapshot.
    /// The dispatch runs post-lock because the wake observer
    /// (`RealtimeSocketFreshnessWake`) uses `try_send` on an mpsc that
    /// doesn't re-enter the authority, but keeping dispatch outside
    /// the lock is uniform with the session-context seam and guards
    /// against future observers that might.
    fn apply_idempotent_with_freshness_effects(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<bool, DslTransitionError> {
        type FreshnessEmission = (mm_dsl::RealtimeProjectionFreshness, u64);
        let sampled: Option<(
            Arc<dyn RealtimeProjectionFreshnessObserver>,
            Vec<FreshnessEmission>,
        )> = match self
            .dsl
            .apply_input_with_effects_and_sample(input, context, |effects| {
                let observer_opt = self
                    .freshness_observer
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .and_then(Weak::upgrade);
                let observer = observer_opt?;
                let emissions: Vec<FreshnessEmission> = effects
                    .iter()
                    .filter_map(|effect| match effect {
                        mm_dsl::MeerkatMachineEffect::RealtimeProjectionFreshnessChanged {
                            new_freshness,
                            frontier_ms,
                        } => Some((*new_freshness, *frontier_ms)),
                        _ => None,
                    })
                    .collect();
                Some((observer, emissions))
            }) {
            Ok(sampled) => sampled,
            Err(err) if err.is_guard_rejected() => return Ok(false),
            Err(err) => return Err(err),
        };
        if let Some((observer, emissions)) = sampled {
            for (new_freshness, frontier_ms) in emissions {
                observer.on_realtime_projection_freshness_changed(
                    map_freshness(new_freshness),
                    frontier_ms,
                );
            }
        }
        Ok(true)
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

fn map_freshness(raw: mm_dsl::RealtimeProjectionFreshness) -> RealtimeProjectionFreshness {
    match raw {
        mm_dsl::RealtimeProjectionFreshness::Clean => RealtimeProjectionFreshness::Clean,
        mm_dsl::RealtimeProjectionFreshness::StaleDeferred => {
            RealtimeProjectionFreshness::StaleDeferred
        }
        mm_dsl::RealtimeProjectionFreshness::StaleImmediate => {
            RealtimeProjectionFreshness::StaleImmediate
        }
    }
}

fn map_policy(raw: mm_dsl::RealtimeReconnectPolicy) -> RealtimeReconnectPolicy {
    match raw {
        mm_dsl::RealtimeReconnectPolicy::CleanExit => RealtimeReconnectPolicy::CleanExit,
        mm_dsl::RealtimeReconnectPolicy::ReattachAndRecover => {
            RealtimeReconnectPolicy::ReattachAndRecover
        }
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

    // ---- Projection freshness (dogma round 2, U-C) ----

    fn projection_advance_observed(&self, advanced_at_ms: u64) -> Result<bool, DslTransitionError> {
        self.apply_idempotent_with_freshness_effects(
            mm_dsl::MeerkatMachineInput::RealtimeProjectionAdvanceObserved { advanced_at_ms },
            "RealtimeProductTurnHandle::projection_advance_observed",
        )
    }

    fn projection_refreshed(&self, observed_ms: u64) -> Result<bool, DslTransitionError> {
        self.apply_idempotent_with_freshness_effects(
            mm_dsl::MeerkatMachineInput::RealtimeProjectionRefreshed { observed_ms },
            "RealtimeProductTurnHandle::projection_refreshed",
        )
    }

    fn projection_baseline_observed(&self, observed_ms: u64) -> Result<bool, DslTransitionError> {
        self.apply_idempotent_with_freshness_effects(
            mm_dsl::MeerkatMachineInput::RealtimeProjectionBaselineObserved { observed_ms },
            "RealtimeProductTurnHandle::projection_baseline_observed",
        )
    }

    fn projection_reset(&self, baseline_ms: u64) -> Result<bool, DslTransitionError> {
        self.apply_idempotent_with_freshness_effects(
            mm_dsl::MeerkatMachineInput::RealtimeProjectionReset { baseline_ms },
            "RealtimeProductTurnHandle::projection_reset",
        )
    }

    fn projection_freshness(&self) -> RealtimeProjectionFreshness {
        map_freshness(self.dsl.snapshot_state().realtime_projection_freshness)
    }

    fn projection_frontier_ms(&self) -> u64 {
        self.dsl.snapshot_state().realtime_projection_frontier_ms
    }

    fn install_projection_freshness_observer(
        &self,
        observer: Arc<dyn RealtimeProjectionFreshnessObserver>,
    ) {
        *self
            .freshness_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&observer));
    }

    fn install_projection_freshness_observer_with_snapshot(
        &self,
        observer: Arc<dyn RealtimeProjectionFreshnessObserver>,
    ) -> (RealtimeProjectionFreshness, u64) {
        self.dsl.with_state_lock(|state| {
            *self
                .freshness_observer
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::downgrade(&observer));
            (
                map_freshness(state.realtime_projection_freshness),
                state.realtime_projection_frontier_ms,
            )
        })
    }

    // ---- Reconnect policy (dogma round 2, U-C) ----

    fn classify_client_input_submitted(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ClassifyRealtimeClientInputSubmitted,
            "RealtimeProductTurnHandle::classify_client_input_submitted",
        )
    }

    fn classify_mid_turn_activity(&self) -> Result<bool, DslTransitionError> {
        self.apply_idempotent(
            mm_dsl::MeerkatMachineInput::ClassifyRealtimeMidTurnActivity,
            "RealtimeProductTurnHandle::classify_mid_turn_activity",
        )
    }

    fn classify_turn_terminated(&self) -> Result<bool, DslTransitionError> {
        // Fold in the freshness-promotion observer dispatch so the turn-
        // terminated classification + any `StaleDeferred → StaleImmediate`
        // promotion arrive on the freshness observer under the same lock.
        self.apply_idempotent_with_freshness_effects(
            mm_dsl::MeerkatMachineInput::ClassifyRealtimeTurnTerminated,
            "RealtimeProductTurnHandle::classify_turn_terminated",
        )
    }

    fn reconnect_policy_on_clean_close(&self) -> RealtimeReconnectPolicy {
        map_policy(self.dsl.snapshot_state().realtime_reconnect_policy)
    }
}
