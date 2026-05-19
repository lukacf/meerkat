// @generated — protocol helpers for `mob_external_peer_trust_repair`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerTrustRepairRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{ExternalPeerEdge, MobMachineEffect, PeerId};

#[derive(Debug, Clone)]
pub struct MobExternalPeerTrustRepairObligation {
    edge: ExternalPeerEdge,
    peer_id: PeerId,
    epoch: u64,
}

impl MobExternalPeerTrustRepairObligation {
    pub fn edge(&self) -> &ExternalPeerEdge {
        &self.edge
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

pub fn extract_obligations(
    effects: &[MobMachineEffect],
) -> Vec<MobExternalPeerTrustRepairObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerTrustRepairRequested {
                edge,
                peer_id,
                epoch,
            } => Some(MobExternalPeerTrustRepairObligation {
                edge: edge.clone(),
                peer_id: peer_id.clone(),
                epoch: *epoch,
            }),
            _ => None,
        })
        .collect()
}

fn validate_expected_peer(
    context: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{context} peer id {actual:?} does not match expected mutation peer id {expected:?}"
        ))
    }
}

pub fn repair_authority_for_peer(
    obligation: &MobExternalPeerTrustRepairObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MobMachineExternalPeerRepair",
        obligation.peer_id.0.as_str(),
        expected_peer_id,
    )?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_mob_machine_peer_repair(
        obligation.peer_id.0.clone(),
        obligation.epoch,
    )
}
