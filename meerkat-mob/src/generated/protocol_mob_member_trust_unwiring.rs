// @generated — protocol helpers for `mob_member_trust_unwiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: MemberTrustUnwiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId, WiringEdge,
};

#[derive(Debug, Clone)]
pub struct MobMemberTrustUnwiringObligation {
    edge: WiringEdge,
    a_peer_id: PeerId,
    b_peer_id: PeerId,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl MobMemberTrustUnwiringObligation {
    pub fn edge(&self) -> &WiringEdge {
        &self.edge
    }

    pub fn a_peer_id(&self) -> &PeerId {
        &self.a_peer_id
    }

    pub fn b_peer_id(&self) -> &PeerId {
        &self.b_peer_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for MobMemberTrustUnwiringObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource for MobMemberTrustUnwiringObligation {
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustUnwiring
    }

    fn authorize_comms_trust_authority(
        &self,
        request: &meerkat_core::comms::GeneratedCommsTrustAuthorityRequest<'_>,
    ) -> Result<meerkat_core::comms::GeneratedCommsTrustAuthorityGrant, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(request.operation(), Operation::PublicRemove) {
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
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(request, self.epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring))
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobMemberTrustUnwiringObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberTrustUnwiringRequested {
                edge,
                a_peer_id,
                b_peer_id,
                epoch,
            } => Some(MobMemberTrustUnwiringObligation {
                edge: edge.clone(),
                a_peer_id: a_peer_id.clone(),
                b_peer_id: b_peer_id.clone(),
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
    obligation: &'a MobMemberTrustUnwiringObligation,
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
    obligation: &'a MobMemberTrustUnwiringObligation,
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

pub fn unwiring_authority_for_identity(
    obligation: &MobMemberTrustUnwiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_remove(
        obligation,
        peer_id.to_owned(),
    )
}
