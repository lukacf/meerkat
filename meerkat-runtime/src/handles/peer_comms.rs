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
    /// Construct a handle backed by a fresh DSL authority.
    pub fn new() -> Self {
        Self {
            dsl: Arc::new(HandleDslAuthority::new()),
        }
    }
}

impl Default for RuntimePeerCommsHandle {
    fn default() -> Self {
        Self::new()
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
