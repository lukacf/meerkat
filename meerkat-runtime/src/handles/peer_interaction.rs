//! Runtime impl of [`meerkat_core::handles::PeerInteractionHandle`] (W1-A).
//!
//! Routes peer request/response lifecycle events into the session's
//! MeerkatMachine DSL (`pending_peer_requests` / `inbound_peer_requests`
//! substate maps) and fans the emitted `PeerInteractionCleanup` effects
//! out to the installed shell-side observer, so the subscriber / stream
//! registries update causally — the map is a strict projection of DSL
//! truth, not shadow state that happens to be updated lexically near each
//! terminal transition.

use std::sync::{Arc, RwLock, Weak};

use meerkat_core::handles::{
    DslTransitionError, PeerInteractionCleanupObserver, PeerInteractionHandle,
    PeerTerminalDisposition as CorePeerDisposition,
};
use meerkat_core::peer_correlation::{
    InboundPeerRequestState as CoreInboundState, OutboundPeerRequestState as CoreOutboundState,
    PeerCorrelationId,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`PeerInteractionHandle`] impl.
///
/// Every trait method routes to the corresponding DSL input on the session's
/// shared MeerkatMachine authority. After the transition lands, emitted
/// effects are scanned for `PeerInteractionCleanup` and dispatched to the
/// installed [`PeerInteractionCleanupObserver`] (if any) — closing the
/// "terminal transition → effect → shell projection cleanup" loop.
///
/// The cleanup observer is held as a `Weak` reference. In production the
/// observer is the session's `CommsRuntime`, which in turn holds a strong
/// `Arc<dyn PeerInteractionHandle>` to this struct; storing the observer
/// strongly would create a cycle that prevents `CommsRuntime::drop` from
/// firing on session teardown (dropped listeners, leaked session-identity
/// claims, zombie `InprocRegistry` entries). `Weak` breaks the cycle —
/// once the runtime drops, `upgrade()` returns `None` and subsequent
/// effect dispatches become no-ops, which is the desired semantics
/// post-teardown.
pub struct RuntimePeerInteractionHandle {
    dsl: Arc<HandleDslAuthority>,
    cleanup_observer: RwLock<Option<Weak<dyn PeerInteractionCleanupObserver>>>,
}

impl std::fmt::Debug for RuntimePeerInteractionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let observer_tag = self
            .cleanup_observer
            .read()
            .ok()
            .as_deref()
            .and_then(|o| o.as_ref().map(|_| "<observer>"));
        f.debug_struct("RuntimePeerInteractionHandle")
            .field("dsl", &self.dsl)
            .field("cleanup_observer", &observer_tag)
            .finish()
    }
}

impl RuntimePeerInteractionHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            cleanup_observer: RwLock::new(None),
        }
    }

    /// Construct a handle backed by an ephemeral DSL authority (tests /
    /// legacy recovery paths).
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }

    fn apply_input_and_dispatch_cleanup(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &'static str,
    ) -> Result<(), DslTransitionError> {
        let effects = self.dsl.apply_input_with_effects(input, context)?;
        // Upgrade the weak observer once per apply; if the owner has been
        // dropped, the cleanup notification is a no-op by design.
        let observer_opt = self
            .cleanup_observer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(observer) = observer_opt {
            for effect in effects {
                if let mm_dsl::MeerkatMachineEffect::PeerInteractionCleanup { corr_id } = effect {
                    match dsl_corr_id_to_core(corr_id.clone()) {
                        Some(core_id) => observer.on_peer_interaction_cleanup(core_id),
                        None => tracing::error!(
                            raw = %corr_id.0,
                            context = context,
                            "PeerInteractionCleanup: DSL emitted a corr_id that is not a valid UUID — broken invariant; skipping observer dispatch"
                        ),
                    }
                }
            }
        }
        Ok(())
    }
}

fn dsl_corr_id_to_core(dsl_id: mm_dsl::PeerCorrelationId) -> Option<PeerCorrelationId> {
    // The DSL key is always produced by `From<PeerCorrelationId> for
    // mm_dsl::PeerCorrelationId`, which stringifies a UUID. Parse must
    // succeed on every canonical path; a parse failure here is a broken
    // invariant, not a recoverable condition. Return `None` so the caller
    // skips observer dispatch and logs — silently substituting nil would
    // cross-contaminate any real `corr_id 0` event.
    uuid::Uuid::parse_str(&dsl_id.0)
        .ok()
        .map(PeerCorrelationId::from_uuid)
}

impl PeerInteractionHandle for RuntimePeerInteractionHandle {
    fn request_sent(
        &self,
        corr_id: PeerCorrelationId,
        to: String,
    ) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestSent {
                corr_id: corr_id.into(),
                to,
            },
            "PeerInteractionHandle::request_sent",
        )
    }

    fn response_progress(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseProgressArrived {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::response_progress",
        )
    }

    fn response_terminal(
        &self,
        corr_id: PeerCorrelationId,
        disposition: CorePeerDisposition,
    ) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseTerminalArrived {
                corr_id: corr_id.into(),
                disposition: disposition.into(),
            },
            "PeerInteractionHandle::response_terminal",
        )
    }

    fn request_timed_out(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestTimedOut {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_timed_out",
        )
    }

    fn request_received(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerRequestReceived {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_received",
        )
    }

    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.apply_input_and_dispatch_cleanup(
            mm_dsl::MeerkatMachineInput::PeerResponseReplied {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::response_replied",
        )
    }

    fn outbound_state(&self, corr_id: PeerCorrelationId) -> Option<CoreOutboundState> {
        let dsl_key: mm_dsl::PeerCorrelationId = corr_id.into();
        self.dsl
            .snapshot_state()
            .pending_peer_requests
            .get(&dsl_key)
            .copied()
            .map(Into::into)
    }

    fn inbound_state(&self, corr_id: PeerCorrelationId) -> Option<CoreInboundState> {
        let dsl_key: mm_dsl::PeerCorrelationId = corr_id.into();
        self.dsl
            .snapshot_state()
            .inbound_peer_requests
            .get(&dsl_key)
            .copied()
            .map(Into::into)
    }

    fn install_cleanup_observer(&self, observer: Arc<dyn PeerInteractionCleanupObserver>) {
        // Downgrade to a `Weak` so this handle does not keep the observer
        // (typically the session's `CommsRuntime`) alive. The caller retains
        // the canonical strong `Arc` via its own field; when the runtime is
        // dropped, the weak here fails to upgrade and cleanup dispatch
        // becomes a no-op — matching the "post-teardown, no more work"
        // semantics the shell-side projection expects.
        *self
            .cleanup_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&observer));
    }
}
