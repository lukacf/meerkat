// @generated — protocol helpers for `mob_external_peer_reciprocal_trust`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerReciprocalTrustRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{ExternalPeerEdge, ExternalPeerKey, MobMachineEffect, PeerId};

#[derive(Debug, Clone)]
pub struct MobExternalPeerReciprocalTrustObligation {
    pub key: ExternalPeerKey,
    pub edge: ExternalPeerEdge,
    pub peer_id: PeerId,
    pub epoch: u64,
}

pub fn extract_obligations(
    effects: &[MobMachineEffect],
) -> Vec<MobExternalPeerReciprocalTrustObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerReciprocalTrustRequested {
                key,
                edge,
                peer_id,
                epoch,
            } => Some(MobExternalPeerReciprocalTrustObligation {
                key: key.clone(),
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

pub fn reciprocal_wiring_authority_for_peer(
    obligation: &MobExternalPeerReciprocalTrustObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MobMachineExternalPeerReciprocalWiring",
        obligation.peer_id.0.as_str(),
        expected_peer_id,
    )?;
    Ok(
        meerkat_core::comms::CommsTrustMutationAuthority::from_generated_mob_machine_peer_wiring(
            obligation.peer_id.0.clone(),
            obligation.epoch,
        ),
    )
}
