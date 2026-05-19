// @generated — protocol helpers for `comms_trust_reconcile`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: CommsTrustReconcileRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};

#[derive(Clone)]
pub struct PeerProjectionFreshnessAuthority {
    authority: Option<
        std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>,
    >,
}

impl std::fmt::Debug for PeerProjectionFreshnessAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerProjectionFreshnessAuthority")
            .field("present", &self.authority.is_some())
            .finish()
    }
}

impl PeerProjectionFreshnessAuthority {
    pub fn from_authority(
        authority: std::sync::Arc<
            std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>,
        >,
    ) -> Self {
        Self {
            authority: Some(authority),
        }
    }

    fn missing() -> Self {
        Self { authority: None }
    }

    fn validate_peer_projection_epoch(&self, expected_epoch: u64) -> Result<(), String> {
        let Some(authority) = &self.authority else {
            return Err("generated peer projection freshness authority is absent".to_string());
        };
        let guard = authority.lock().map_err(|_| {
            "generated peer projection freshness authority was poisoned".to_string()
        })?;
        let current_epoch = guard.state().peer_projection_epoch;
        if current_epoch == expected_epoch {
            Ok(())
        } else {
            Err(format!(
                "stale generated peer projection trust obligation at epoch {expected_epoch} (current {current_epoch})"
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommsTrustReconcileObligation {
    peer_projection_epoch: u64,
    direct_peer_endpoints: std::collections::BTreeSet<PeerEndpoint>,
    mob_overlay_peer_endpoints: std::collections::BTreeSet<PeerEndpoint>,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    peer_projection_freshness_authority: PeerProjectionFreshnessAuthority,
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

fn trusted_peer_descriptor_from_peer_endpoint(
    endpoint: &crate::meerkat_machine::dsl::PeerEndpoint,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.as_str(),
        endpoint.signing_key.0,
        endpoint.address.0.as_str(),
    )
}

fn trusted_peer_descriptor_for_request(
    obligation: &CommsTrustReconcileObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    let mut matches = obligation
        .direct_peer_endpoints
        .iter()
        .chain(obligation.mob_overlay_peer_endpoints.iter())
        .filter(|endpoint| endpoint.peer_id.0 == peer_id);
    let Some(endpoint) = matches.next() else {
        return Err(format!(
            "MeerkatMachine peer projection did not request trust for peer {peer_id:?}"
        ));
    };
    if matches.next().is_some() {
        return Err(format!(
            "MeerkatMachine peer projection has ambiguous endpoint descriptors for peer {peer_id:?}"
        ));
    }
    trusted_peer_descriptor_from_peer_endpoint(endpoint)
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
        self.peer_projection_freshness_authority
            .validate_peer_projection_epoch(self.peer_projection_epoch)?;
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
        if matches!(
            request.operation(),
            Operation::PublicAdd | Operation::PrivateAdd
        ) {
            let peer_descriptor = trusted_peer_descriptor_for_request(self, request.peer_id())?;
            let grant = meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new_add(request, self.peer_projection_epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection, peer_descriptor)?;
            return Ok(grant);
        }
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(request, self.peer_projection_epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection))
    }
}

pub fn extract_obligations(
    transition: &MeerkatMachineTransition,
) -> Vec<CommsTrustReconcileObligation> {
    extract_obligations_with_freshness(transition, PeerProjectionFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MeerkatMachineTransition,
    peer_projection_freshness_authority: PeerProjectionFreshnessAuthority,
) -> Vec<CommsTrustReconcileObligation> {
    transition
        .effects()
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
                peer_projection_freshness_authority: peer_projection_freshness_authority.clone(),
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
        trusted_peer_descriptor_from_peer_endpoint(endpoint)?,
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
