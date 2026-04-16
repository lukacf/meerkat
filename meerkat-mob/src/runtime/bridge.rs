//! `MobMemberRuntimeBridge` — typed protocol boundary between MobMachine and
//! the MeerkatMachine instances it supervises.
//!
//! All member lifecycle operations go through this trait. The mob crate owns
//! the contract; the runtime crate implements it.
//!
//! - **Local members:** implemented by `LocalMobRuntimeBridge` wrapping
//!   `Arc<MeerkatMachine>`.
//! - **Remote members:** implemented by `RemoteMobRuntimeBridge` sending
//!   typed [`BridgeCommand`](meerkat_contracts::BridgeCommand) envelopes over
//!   comms.

use super::bridge_protocol::{
    BridgeAck, BridgeBindResponse, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeMemberRuntimeState, BridgeObservationResponse, BridgePeerSpec, BridgeRetireResponse,
};
use crate::error::MobError;
use async_trait::async_trait;
use meerkat_core::types::{ContentInput, HandlingMode};

/// Protocol boundary between MobMachine and the MeerkatMachine instances
/// it supervises. All member lifecycle operations go through this trait.
///
/// Local members: implemented by a wrapper around MeerkatMachine.
/// Remote members: implemented by a comms-based protocol client.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobMemberRuntimeBridge: Send + Sync {
    // --- Binding ---

    /// Bind a remote runtime to this mob. Stages the runtime, authorizes the
    /// supervisor, commits the binding, and returns a `BridgeBindResponse`.
    async fn bind_member(
        &self,
        expected_peer_id: &str,
        expected_address: &str,
    ) -> Result<BridgeBindResponse, MobError>;

    // --- Supervisor authority ---

    /// Authorize (or re-authorize) the supervisor for this member.
    async fn authorize_supervisor(&self) -> Result<BridgeAck, MobError>;

    /// Revoke the supervisor's authority over this member.
    async fn revoke_supervisor(&self) -> Result<BridgeAck, MobError>;

    // --- Input delivery ---

    /// Deliver one logical input to the member. Duplicate `input_id` values
    /// join the same in-flight admission or return the original acceptance
    /// result.
    async fn deliver_member_input(
        &self,
        input_id: &str,
        content: ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<BridgeDeliveryResponse, MobError>;

    // --- Observation ---

    /// Return a partial-tolerant observation snapshot.
    async fn observe_member(&self) -> Result<BridgeObservationResponse, MobError>;

    // --- Lifecycle commands ---

    /// Interrupt the member's in-flight run.
    async fn interrupt_member(&self) -> Result<BridgeAck, MobError>;

    /// Retire the member's runtime (drain queued work, archive session).
    async fn retire_member(&self) -> Result<BridgeRetireResponse, MobError>;

    /// Destroy the member's runtime (terminal, no recovery).
    async fn destroy_member(&self) -> Result<BridgeDestroyResponse, MobError>;

    // --- Peer wiring ---

    /// Add a trusted peer to the member's comms runtime.
    async fn wire_member(&self, peer_spec: BridgePeerSpec) -> Result<BridgeAck, MobError>;

    /// Remove a trusted peer from the member's comms runtime.
    async fn unwire_member(&self, peer_spec: BridgePeerSpec) -> Result<BridgeAck, MobError>;
}

/// Observe whether a member is in a terminal runtime state.
pub fn observation_is_terminal(observation: &BridgeObservationResponse) -> bool {
    matches!(
        observation.state,
        BridgeMemberRuntimeState::Retired
            | BridgeMemberRuntimeState::Stopped
            | BridgeMemberRuntimeState::Destroyed
    )
}
