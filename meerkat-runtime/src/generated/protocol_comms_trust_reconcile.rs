// @generated — protocol helpers for `comms_trust_reconcile`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: CommsTrustReconcileRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};

#[derive(Debug, Clone)]
pub struct CommsTrustReconcileObligation {
    peer_projection_epoch: u64,
    direct_peer_endpoints: std::collections::BTreeSet<PeerEndpoint>,
    mob_overlay_peer_endpoints: std::collections::BTreeSet<PeerEndpoint>,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl CommsTrustReconcileObligation {
    pub fn peer_projection_epoch(&self) -> u64 {
        self.peer_projection_epoch
    }

    pub fn direct_peer_endpoints(&self) -> &std::collections::BTreeSet<PeerEndpoint> {
        &self.direct_peer_endpoints
    }

    pub fn mob_overlay_peer_endpoints(&self) -> &std::collections::BTreeSet<PeerEndpoint> {
        &self.mob_overlay_peer_endpoints
    }
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for CommsTrustReconcileObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource for CommsTrustReconcileObligation {
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection
    }

    fn authorize_comms_trust_authority(
        &self,
        request: &meerkat_core::comms::GeneratedCommsTrustAuthorityRequest<'_>,
    ) -> Result<meerkat_core::comms::GeneratedCommsTrustAuthorityGrant, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(
            request.operation(),
            Operation::PublicAdd | Operation::PublicRemove
        ) {
            return Err(format!(
                "generated comms trust source {:?} cannot authorize operation {:?}",
                self.comms_trust_authority_source_kind(),
                request.operation()
            ));
        }
        match request.operation() {
            Operation::PublicAdd => {
                let requested = self
                    .direct_peer_endpoints
                    .iter()
                    .chain(self.mob_overlay_peer_endpoints.iter())
                    .any(|endpoint| endpoint.peer_id.0 == request.peer_id());
                if !requested {
                    return Err(format!(
                        "MeerkatMachine peer projection did not request trust for peer {:?}",
                        request.peer_id()
                    ));
                }
            }
            Operation::PublicRemove => {
                let still_requested = self
                    .direct_peer_endpoints
                    .iter()
                    .chain(self.mob_overlay_peer_endpoints.iter())
                    .any(|endpoint| endpoint.peer_id.0 == request.peer_id());
                if still_requested {
                    return Err(format!(
                        "MeerkatMachine peer projection still requests trust for peer {:?}",
                        request.peer_id()
                    ));
                }
            }
            _ => unreachable!("operation checked above"),
        }
        let claim_key = format!("{:?}:{}", request.operation(), request.peer_id());
        let mut claims = self.comms_trust_authority_claims.lock().map_err(|_| {
            "generated comms trust authority source claims were poisoned".to_string()
        })?;
        if !claims.insert(claim_key) {
            return Err(format!(
                "generated comms trust authority source already minted {:?} for peer {:?}",
                request.operation(),
                request.peer_id()
            ));
        }
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(
            request,
            self.peer_projection_epoch,
        ))
    }
}

pub fn extract_obligations(
    transition: &MeerkatMachineTransition,
) -> Vec<CommsTrustReconcileObligation> {
    transition
        .effects
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::CommsTrustReconcileRequested {
                peer_projection_epoch,
                direct_peer_endpoints,
                mob_overlay_peer_endpoints,
            } => Some(CommsTrustReconcileObligation {
                peer_projection_epoch: *peer_projection_epoch,
                direct_peer_endpoints: direct_peer_endpoints.clone(),
                mob_overlay_peer_endpoints: mob_overlay_peer_endpoints.clone(),
                comms_trust_authority_claims: Default::default(),
            }),
            _ => None,
        })
        .collect()
}

pub fn effective_peers(
    obligation: &CommsTrustReconcileObligation,
) -> std::collections::BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint> {
    obligation
        .direct_peer_endpoints
        .iter()
        .chain(obligation.mob_overlay_peer_endpoints.iter())
        .cloned()
        .collect()
}

pub fn peer_projection_epoch(obligation: &CommsTrustReconcileObligation) -> u64 {
    obligation.peer_projection_epoch
}

pub fn authority_for_endpoint(
    obligation: &CommsTrustReconcileObligation,
    endpoint: &crate::meerkat_machine::dsl::PeerEndpoint,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    if !effective_peers(obligation).contains(endpoint) {
        return Err(format!(
            "MeerkatMachine peer projection did not request trust for peer '{}'",
            endpoint.peer_id.0
        ));
    }
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_add(
        obligation,
        endpoint.peer_id.0.clone(),
    )
}

pub fn removal_authority_for_peer_id(
    obligation: &CommsTrustReconcileObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    if effective_peers(obligation)
        .iter()
        .any(|endpoint| endpoint.peer_id.0 == peer_id)
    {
        return Err(format!(
            "MeerkatMachine peer projection still requests trust for peer '{peer_id}'"
        ));
    }
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_remove(
        obligation,
        peer_id.to_string(),
    )
}
