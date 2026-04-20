//! Runtime impl of [`meerkat_core::handles::InteractionStreamHandle`] (U6).
//!
//! Routes the interaction-stream lifecycle (`Reserved` / `Attached` /
//! `Completed` / `Expired` / `ClosedEarly`) into the session's MeerkatMachine
//! DSL `interaction_streams` substate map and fans the emitted
//! `InteractionStreamCleanup` effects out to the installed shell-side
//! observer, so the comms runtime's `interaction_stream_registry` becomes a
//! pure projection of DSL truth — no shadow `state` field, no shell-side
//! CAS, no TTL meaning hidden in registry maps.

use std::sync::{Arc, RwLock, Weak};

use meerkat_core::handles::{
    DslTransitionError, InteractionStreamCleanupObserver, InteractionStreamHandle,
};
use meerkat_core::peer_correlation::{
    InteractionStreamState as CoreInteractionStreamState, PeerCorrelationId,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`InteractionStreamHandle`] impl.
///
/// Every trait method routes to the corresponding DSL input on the session's
/// shared MeerkatMachine authority. After the transition lands, emitted
/// effects are scanned for `InteractionStreamCleanup` and dispatched to the
/// installed [`InteractionStreamCleanupObserver`] (if any), closing the
/// "terminal transition → effect → shell projection cleanup" loop.
///
/// The observer is held as a `Weak` reference because the canonical owner is
/// the session's `CommsRuntime`, which in turn holds a strong handle pointer;
/// storing the observer strongly would create a cycle preventing
/// `CommsRuntime::drop` from firing on session teardown. `Weak::upgrade`
/// returning `None` after teardown makes cleanup dispatch a no-op, which is
/// the desired post-shutdown semantics.
pub struct RuntimeInteractionStreamHandle {
    dsl: Arc<HandleDslAuthority>,
    cleanup_observer: RwLock<Option<Weak<dyn InteractionStreamCleanupObserver>>>,
}

impl std::fmt::Debug for RuntimeInteractionStreamHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let observer_tag = self
            .cleanup_observer
            .read()
            .ok()
            .as_deref()
            .and_then(|o| o.as_ref().map(|_| "<observer>"));
        f.debug_struct("RuntimeInteractionStreamHandle")
            .field("dsl", &self.dsl)
            .field("cleanup_observer", &observer_tag)
            .finish()
    }
}

impl RuntimeInteractionStreamHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            cleanup_observer: RwLock::new(None),
        }
    }

    /// Construct a handle backed by an ephemeral DSL authority (tests / WASM /
    /// standalone fallback).
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }

    fn apply_input_and_dispatch_cleanup(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        let effects = self.dsl.apply_input_with_effects(input, context)?;
        let observer_opt = self
            .cleanup_observer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(observer) = observer_opt {
            for effect in effects {
                if let mm_dsl::MeerkatMachineEffect::InteractionStreamCleanup { corr_id } = effect {
                    match dsl_corr_id_to_core(corr_id.clone()) {
                        Some(core_id) => observer.on_interaction_stream_cleanup(core_id),
                        None => tracing::error!(
                            raw = %corr_id.0,
                            context = context,
                            "InteractionStreamCleanup: DSL emitted a corr_id that is not a valid UUID — broken invariant; skipping observer dispatch"
                        ),
                    }
                }
            }
        }
        Ok(())
    }
}

fn dsl_corr_id_to_core(dsl_id: mm_dsl::PeerCorrelationId) -> Option<PeerCorrelationId> {
    uuid::Uuid::parse_str(&dsl_id.0)
        .ok()
        .map(PeerCorrelationId::from_uuid)
}

impl InteractionStreamHandle for RuntimeInteractionStreamHandle {
    fn reserved(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::InteractionStreamReserved {
                corr_id: corr_id.into(),
            },
            "InteractionStreamHandle::reserved",
        )
    }

    fn attached(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::InteractionStreamAttached {
                corr_id: corr_id.into(),
            },
            "InteractionStreamHandle::attached",
        )
    }

    fn completed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::InteractionStreamCompleted {
                corr_id: corr_id.into(),
            },
            "InteractionStreamHandle::completed",
        )
    }

    fn expired(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::InteractionStreamExpired {
                corr_id: corr_id.into(),
            },
            "InteractionStreamHandle::expired",
        )
    }

    fn closed_early(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::InteractionStreamClosedEarly {
                corr_id: corr_id.into(),
            },
            "InteractionStreamHandle::closed_early",
        )
    }

    fn state(&self, corr_id: PeerCorrelationId) -> Option<CoreInteractionStreamState> {
        let dsl_key: mm_dsl::PeerCorrelationId = corr_id.into();
        let snapshot = self.dsl.snapshot_state();
        // Disjoint-set encoding (matches the DSL `interaction_stream_disjoint`
        // discipline): a corr_id is in at most one of the two active sets.
        // Terminal states (`Completed` / `Expired` / `ClosedEarly`) leave both
        // sets and are never observable here — they surface only via the
        // `InteractionStreamStateChanged` effect, like the peer-correlation
        // sibling enums.
        if snapshot.attached_interaction_streams.contains(&dsl_key) {
            Some(CoreInteractionStreamState::Attached)
        } else if snapshot.reserved_interaction_streams.contains(&dsl_key) {
            Some(CoreInteractionStreamState::Reserved)
        } else {
            None
        }
    }

    fn install_cleanup_observer(&self, observer: Arc<dyn InteractionStreamCleanupObserver>) {
        *self
            .cleanup_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&observer));
    }
}
