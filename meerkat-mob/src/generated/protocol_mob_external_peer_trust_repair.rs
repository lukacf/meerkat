// @generated — protocol helpers for `mob_external_peer_trust_repair`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerTrustRepairRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    ExternalPeerEdge, MobMachineEffect, MobMachineTransition, PeerId,
};

#[derive(Debug, Clone)]
pub struct MobExternalPeerTrustRepairObligation {
    edge: ExternalPeerEdge,
    peer_id: PeerId,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
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

fn trusted_peer_descriptor_from_external_endpoint(
    endpoint: &crate::machines::mob_machine::ExternalPeerEndpoint,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        endpoint.name.0.clone(),
        endpoint.peer_id.0.as_str(),
        endpoint.signing_key.0,
        endpoint.address.0.as_str(),
    )
}

fn trusted_peer_descriptor_for_request(
    obligation: &MobExternalPeerTrustRepairObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    if obligation.edge.endpoint.peer_id.0 != peer_id {
        return Err(format!(
            "MobMachine external trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
            obligation.edge.endpoint.peer_id.0
        ));
    }
    trusted_peer_descriptor_from_external_endpoint(&obligation.edge.endpoint)
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for MobExternalPeerTrustRepairObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource
    for MobExternalPeerTrustRepairObligation
{
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustRepair
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
            return meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new_add(
                request,
                self.epoch,
                peer_descriptor,
            );
        }
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(
            request, self.epoch,
        ))
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobExternalPeerTrustRepairObligation> {
    transition
        .effects()
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
                comms_trust_authority_claims: Default::default(),
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
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_add(
        obligation,
        trusted_peer_descriptor_for_request(obligation, expected_peer_id)?,
    )
}
