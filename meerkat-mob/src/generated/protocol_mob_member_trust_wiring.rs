// @generated — protocol helpers for `mob_member_trust_wiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: MemberTrustWiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId, WiringEdge,
};

#[derive(Debug, Clone)]
pub struct MobMemberTrustWiringObligation {
    edge: WiringEdge,
    a_peer_id: PeerId,
    b_peer_id: PeerId,
    a_endpoint: MemberPeerEndpoint,
    b_endpoint: MemberPeerEndpoint,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl MobMemberTrustWiringObligation {
    pub fn edge(&self) -> &WiringEdge {
        &self.edge
    }

    pub fn a_peer_id(&self) -> &PeerId {
        &self.a_peer_id
    }

    pub fn b_peer_id(&self) -> &PeerId {
        &self.b_peer_id
    }

    pub fn a_endpoint(&self) -> &MemberPeerEndpoint {
        &self.a_endpoint
    }

    pub fn b_endpoint(&self) -> &MemberPeerEndpoint {
        &self.b_endpoint
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
    obligation: &MobMemberTrustWiringObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    let a_matches = obligation.a_endpoint.peer_id.0 == peer_id;
    let b_matches = obligation.b_endpoint.peer_id.0 == peer_id;
    match (a_matches, b_matches) {
        (true, false) => trusted_peer_descriptor_from_member_endpoint(&obligation.a_endpoint),
        (false, true) => trusted_peer_descriptor_from_member_endpoint(&obligation.b_endpoint),
        (false, false) => Err(format!(
            "MobMachine member trust obligation does not carry requested peer {peer_id:?}"
        )),
        (true, true) => Err(format!(
            "MobMachine member trust obligation has ambiguous endpoint descriptors for peer {peer_id:?}"
        )),
    }
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for MobMemberTrustWiringObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource for MobMemberTrustWiringObligation {
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring
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
        if self.a_peer_id.0 != request.peer_id() && self.b_peer_id.0 != request.peer_id() {
            return Err(format!(
                "MobMachine member trust obligation does not carry requested peer {:?}",
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
) -> Vec<MobMemberTrustWiringObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberTrustWiringRequested {
                edge,
                a_peer_id,
                b_peer_id,
                a_endpoint,
                b_endpoint,
                epoch,
            } => Some(MobMemberTrustWiringObligation {
                edge: edge.clone(),
                a_peer_id: a_peer_id.clone(),
                b_peer_id: b_peer_id.clone(),
                a_endpoint: a_endpoint.clone(),
                b_endpoint: b_endpoint.clone(),
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

fn peer_id_for_identity<'a>(
    obligation: &'a MobMemberTrustWiringObligation,
    identity: &str,
) -> Option<&'a str> {
    if obligation.edge.a.0 == identity {
        Some(obligation.a_peer_id.0.as_str())
    } else if obligation.edge.b.0 == identity {
        Some(obligation.b_peer_id.0.as_str())
    } else {
        None
    }
}

fn required_peer_id_for_identity<'a>(
    obligation: &'a MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<&'a str, String> {
    let Some(actual) = peer_id_for_identity(obligation, identity) else {
        return Err(format!(
            "MobMachine member trust obligation does not cover identity {identity:?}"
        ));
    };
    validate_expected_peer("MobMachineMemberTrust", actual, expected_peer_id)?;
    Ok(actual)
}

pub fn wiring_authority_for_identity(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_add(
        obligation,
        trusted_peer_descriptor_for_request(obligation, peer_id)?,
    )
}

pub fn repair_authority_for_identity(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_add(
        obligation,
        trusted_peer_descriptor_for_request(obligation, peer_id)?,
    )
}
