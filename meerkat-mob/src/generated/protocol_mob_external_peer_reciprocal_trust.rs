// @generated — protocol helpers for `mob_external_peer_reciprocal_trust`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerReciprocalTrustRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    ExternalPeerEdge, ExternalPeerKey, MemberPeerEndpoint, MobMachineEffect, MobMachineTransition,
    PeerId,
};

#[derive(Debug, Clone)]
pub struct MobTopologyFreshnessAuthority {
    topology_epoch: Option<u64>,
}

impl MobTopologyFreshnessAuthority {
    pub fn from_authority(authority: &crate::machines::mob_machine::MobMachineAuthority) -> Self {
        Self {
            topology_epoch: Some(authority.state().topology_epoch),
        }
    }

    fn missing() -> Self {
        Self {
            topology_epoch: None,
        }
    }

    fn validate_topology_epoch(&self, expected_epoch: u64) -> Result<(), String> {
        let Some(current_epoch) = self.topology_epoch else {
            return Err("generated MobMachine topology freshness authority is absent".to_string());
        };
        if current_epoch == expected_epoch {
            Ok(())
        } else {
            Err(format!(
                "stale generated MobMachine trust obligation at epoch {expected_epoch} (current {current_epoch})"
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct MobExternalPeerReciprocalTrustObligation {
    key: ExternalPeerKey,
    edge: ExternalPeerEdge,
    peer_id: PeerId,
    peer_endpoint: MemberPeerEndpoint,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
}

impl MobExternalPeerReciprocalTrustObligation {
    pub fn key(&self) -> &ExternalPeerKey {
        &self.key
    }

    pub fn edge(&self) -> &ExternalPeerEdge {
        &self.edge
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    pub fn peer_endpoint(&self) -> &MemberPeerEndpoint {
        &self.peer_endpoint
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

fn trusted_peer_descriptor_from_member_endpoint(
    endpoint: &crate::machines::mob_machine::MemberPeerEndpoint,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.as_str(),
        endpoint.signing_key.0,
        endpoint.address.0.as_str(),
    )
}

fn trusted_peer_descriptor_for_request(
    obligation: &MobExternalPeerReciprocalTrustObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    if obligation.peer_endpoint.peer_id.0 != peer_id {
        return Err(format!(
            "MobMachine external reciprocal trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
            obligation.peer_endpoint.peer_id.0
        ));
    }
    trusted_peer_descriptor_from_member_endpoint(&obligation.peer_endpoint)
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for MobExternalPeerReciprocalTrustObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource
    for MobExternalPeerReciprocalTrustObligation
{
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerReciprocalTrust
    }

    fn authorize_comms_trust_authority(
        &self,
        request: &meerkat_core::comms::GeneratedCommsTrustAuthorityRequest<'_>,
    ) -> Result<meerkat_core::comms::GeneratedCommsTrustAuthorityGrant, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(request.operation(), Operation::PublicAdd) {
            return Err(format!(
                "generated comms trust source {:?} cannot authorize operation {:?}",
                self.comms_trust_authority_source_kind(),
                request.operation()
            ));
        }
        self.mob_topology_freshness_authority
            .validate_topology_epoch(self.epoch)?;
        if self.peer_id.0 != request.peer_id() {
            return Err(format!(
                "MobMachine external trust obligation peer_id {:?} does not match requested peer {:?}",
                self.peer_id.0,
                request.peer_id()
            ));
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
            let grant = meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new_add(request, self.epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring, peer_descriptor)?;
            return Ok(grant.with_trust_store_peer_id(self.edge.endpoint.peer_id.0.as_str()));
        }
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(request, self.epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring).with_trust_store_peer_id(self.edge.endpoint.peer_id.0.as_str()))
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobExternalPeerReciprocalTrustObligation> {
    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MobMachineTransition,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
) -> Vec<MobExternalPeerReciprocalTrustObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerReciprocalTrustRequested {
                key,
                edge,
                peer_id,
                peer_endpoint,
                epoch,
            } => Some(MobExternalPeerReciprocalTrustObligation {
                key: key.clone(),
                edge: edge.clone(),
                peer_id: peer_id.clone(),
                peer_endpoint: peer_endpoint.clone(),
                epoch: *epoch,
                comms_trust_authority_claims: Default::default(),
                mob_topology_freshness_authority: mob_topology_freshness_authority.clone(),
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
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_add(
        obligation,
        trusted_peer_descriptor_for_request(obligation, expected_peer_id)?,
    )
}
