// @generated — comms trust authority handoff helpers for MeerkatMachine.
// Producer: MeerkatMachine peer projection and supervisor trust effects.

use crate::meerkat_machine::dsl::PeerEndpoint;
use crate::protocol_supervisor_trust_publish::SupervisorTrustPublishObligation;
use crate::protocol_supervisor_trust_revoke::SupervisorTrustRevokeObligation;
use meerkat_core::generated::comms_trust_authority as core_authority;

struct PeerProjectionTrustObligation {
    peer_id: String,
    epoch: u64,
}

impl core_authority::GeneratedMeerkatMachinePeerProjectionHandoff
    for PeerProjectionTrustObligation
{
    fn peer_id(&self) -> &str {
        self.peer_id.as_str()
    }

    fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl core_authority::GeneratedMeerkatMachineSupervisorTrustHandoff
    for SupervisorTrustPublishObligation
{
    fn peer_id(&self) -> &str {
        self.peer_id.as_str()
    }

    fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl core_authority::GeneratedMeerkatMachineSupervisorTrustHandoff
    for SupervisorTrustRevokeObligation
{
    fn peer_id(&self) -> &str {
        self.peer_id.as_str()
    }

    fn epoch(&self) -> u64 {
        self.epoch
    }
}

pub fn peer_projection_handoff(
    endpoint: &PeerEndpoint,
    epoch: u64,
) -> core_authority::MeerkatMachinePeerProjectionHandoff {
    core_authority::MeerkatMachinePeerProjectionHandoff::from_generated_projection(
        &PeerProjectionTrustObligation {
            peer_id: endpoint.peer_id.0.clone(),
            epoch,
        },
    )
}

pub fn supervisor_publish_handoff(
    obligation: &SupervisorTrustPublishObligation,
) -> core_authority::MeerkatMachineSupervisorTrustHandoff {
    core_authority::MeerkatMachineSupervisorTrustHandoff::from_generated_supervisor_publish(
        obligation,
    )
}

pub fn supervisor_revoke_handoff(
    obligation: &SupervisorTrustRevokeObligation,
) -> core_authority::MeerkatMachineSupervisorTrustHandoff {
    core_authority::MeerkatMachineSupervisorTrustHandoff::from_generated_supervisor_revoke(
        obligation,
    )
}
