// @generated — protocol helpers for `mob_external_peer_trust_wiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerTrustWiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    ExternalPeerEdge, MobMachineEffect, MobMachineTransition, PeerId,
};

#[derive(Debug, Clone)]
pub struct MobExternalPeerTrustWiringObligation {
    edge: ExternalPeerEdge,
    peer_id: PeerId,
    epoch: u64,
}

impl MobExternalPeerTrustWiringObligation {
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

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for MobExternalPeerTrustWiringObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource
    for MobExternalPeerTrustWiringObligation
{
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobExternalPeerTrustWiringObligation> {
    transition
        .effects
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerTrustWiringRequested {
                edge,
                peer_id,
                epoch,
            } => Some(MobExternalPeerTrustWiringObligation {
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

pub fn wiring_authority_for_peer(
    obligation: &MobExternalPeerTrustWiringObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MobMachineExternalPeerWiring",
        obligation.peer_id.0.as_str(),
        expected_peer_id,
    )?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_mob_machine_peer_wiring(
        obligation,
        obligation.peer_id.0.clone(),
        obligation.epoch,
    )
}
