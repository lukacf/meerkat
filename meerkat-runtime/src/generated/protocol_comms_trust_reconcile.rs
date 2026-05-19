// @generated — protocol helpers for `comms_trust_reconcile`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: CommsTrustReconcileRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::meerkat_machine::dsl::MeerkatMachineEffect;

#[derive(Debug, Clone)]
pub struct CommsTrustReconcileObligation {
    pub peer_projection_epoch: u64,
}

pub fn extract_obligations(effects: &[MeerkatMachineEffect]) -> Vec<CommsTrustReconcileObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::CommsTrustReconcileRequested {
                peer_projection_epoch,
            } => Some(CommsTrustReconcileObligation {
                peer_projection_epoch: *peer_projection_epoch,
            }),
            _ => None,
        })
        .collect()
}

pub fn authority_for_endpoint(
    obligation: &CommsTrustReconcileObligation,
    endpoint: &crate::meerkat_machine::dsl::PeerEndpoint,
) -> meerkat_core::comms::CommsTrustMutationAuthority {
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_meerkat_machine_peer_projection(
        endpoint.peer_id.0.clone(),
        obligation.peer_projection_epoch,
    )
}
