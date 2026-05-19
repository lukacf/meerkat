// @generated — protocol helpers for `mob_external_peer_trust_unwiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerTrustUnwiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{ExternalPeerEdge, MobMachineEffect, PeerId};

#[derive(Debug, Clone)]
pub struct MobExternalPeerTrustUnwiringObligation {
    pub edge: ExternalPeerEdge,
    pub peer_id: PeerId,
    pub epoch: u64,
}

pub fn extract_obligations(
    effects: &[MobMachineEffect],
) -> Vec<MobExternalPeerTrustUnwiringObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerTrustUnwiringRequested {
                edge,
                peer_id,
                epoch,
            } => Some(MobExternalPeerTrustUnwiringObligation {
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

pub fn unwiring_authority_for_peer(
    obligation: &MobExternalPeerTrustUnwiringObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MobMachineExternalPeerUnwiring",
        obligation.peer_id.0.as_str(),
        expected_peer_id,
    )?;
    Ok(
        meerkat_core::comms::CommsTrustMutationAuthority::from_generated_mob_machine_peer_unwiring(
            obligation.peer_id.0.clone(),
            obligation.epoch,
        ),
    )
}
