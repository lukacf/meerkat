//! Runtime impl of [`meerkat_core::handles::PeerCommsHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, PeerCommsHandle};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`PeerCommsHandle`] impl.
///
/// Routes every trait method to the corresponding DSL signal / input on a
/// dedicated per-session MeerkatMachine DSL authority.
#[derive(Debug)]
pub struct RuntimePeerCommsHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimePeerCommsHandle {
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

impl PeerCommsHandle for RuntimePeerCommsHandle {
    fn classify_external_envelope(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::ClassifyExternalEnvelope,
            "PeerCommsHandle::classify_external_envelope",
        )
    }

    fn classify_plain_event(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::ClassifyPlainEvent,
            "PeerCommsHandle::classify_plain_event",
        )
    }

    fn set_peer_ingress_context(&self, keep_alive: bool) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SetPeerIngressContext { keep_alive },
            "PeerCommsHandle::set_peer_ingress_context",
        )
    }
}
