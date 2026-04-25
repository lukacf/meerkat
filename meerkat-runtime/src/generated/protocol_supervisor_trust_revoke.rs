// @generated — protocol helpers for `supervisor_trust_revoke`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: RevokeSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, PeerId};

#[derive(Debug, Clone)]
pub struct SupervisorTrustRevokeObligation {
    pub peer_id: PeerId,
    pub epoch: u64,
}

pub fn extract_obligations(
    effects: &[MeerkatMachineEffect],
) -> Vec<SupervisorTrustRevokeObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::RevokeSupervisorTrustEdge { peer_id, epoch } => {
                Some(SupervisorTrustRevokeObligation {
                    peer_id: peer_id.clone(),
                    epoch: *epoch,
                })
            }
            _ => None,
        })
        .collect()
}
