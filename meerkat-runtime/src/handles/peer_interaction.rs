//! Runtime impl of [`meerkat_core::handles::PeerInteractionHandle`] (W1-A).
//!
//! Routes peer request/response lifecycle events into the session's
//! MeerkatMachine DSL (`pending_peer_requests` / `inbound_peer_requests`
//! substate maps). Downstream projection consumers (the comms runtime's
//! subscriber / stream registries) observe the `PeerInteractionCleanup`
//! effect to drop channel handles on terminal transitions — the channels
//! are a pure shell-owned projection of DSL state, not shadow truth.

use std::sync::Arc;

use meerkat_core::handles::{
    DslTransitionError, PeerInteractionHandle, PeerTerminalDisposition as CorePeerDisposition,
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
/// shared MeerkatMachine authority. `PeerTerminalDisposition` on the core
/// trait maps to the DSL's bridging `PeerTerminalDisposition` enum via
/// [`From`].
#[derive(Debug)]
pub struct RuntimePeerInteractionHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimePeerInteractionHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority (tests /
    /// legacy recovery paths).
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl PeerInteractionHandle for RuntimePeerInteractionHandle {
    fn request_sent(
        &self,
        corr_id: PeerCorrelationId,
        to: String,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PeerRequestSent {
                corr_id: corr_id.into(),
                to,
            },
            "PeerInteractionHandle::request_sent",
        )
    }

    fn response_progress(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
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
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PeerResponseTerminalArrived {
                corr_id: corr_id.into(),
                disposition: disposition.into(),
            },
            "PeerInteractionHandle::response_terminal",
        )
    }

    fn request_timed_out(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PeerRequestTimedOut {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_timed_out",
        )
    }

    fn request_received(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PeerRequestReceived {
                corr_id: corr_id.into(),
            },
            "PeerInteractionHandle::request_received",
        )
    }

    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
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
}
