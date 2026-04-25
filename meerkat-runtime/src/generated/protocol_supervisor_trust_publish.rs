// @generated — protocol helpers for `supervisor_trust_publish`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: PublishSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, PeerId};

#[derive(Debug, Clone)]
pub struct SupervisorTrustPublishObligation {
    pub peer_id: PeerId,
    pub name: String,
    pub address: String,
    pub signing_public_key: Option<String>,
    pub epoch: u64,
}

pub fn extract_obligations(
    effects: &[MeerkatMachineEffect],
) -> Vec<SupervisorTrustPublishObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::PublishSupervisorTrustEdge {
                peer_id,
                name,
                address,
                signing_public_key,
                epoch,
            } => Some(SupervisorTrustPublishObligation {
                peer_id: peer_id.clone(),
                name: name.clone(),
                address: address.clone(),
                signing_public_key: signing_public_key.clone(),
                epoch: *epoch,
            }),
            _ => None,
        })
        .collect()
}
