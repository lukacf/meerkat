// @generated — protocol helpers for `mob_member_trust_wiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: MemberTrustWiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{MobMachineEffect, MobMachineTransition, PeerId, WiringEdge};

#[derive(Debug, Clone)]
pub struct MobMemberTrustWiringObligation {
    edge: WiringEdge,
    a_peer_id: PeerId,
    b_peer_id: PeerId,
    epoch: u64,
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

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobMemberTrustWiringObligation> {
    transition
        .effects
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberTrustWiringRequested {
                edge,
                a_peer_id,
                b_peer_id,
                epoch,
            } => Some(MobMemberTrustWiringObligation {
                edge: edge.clone(),
                a_peer_id: a_peer_id.clone(),
                b_peer_id: b_peer_id.clone(),
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
    Ok(
        meerkat_core::comms::CommsTrustMutationAuthority::from_generated_mob_machine_peer_wiring(
            peer_id.to_owned(),
            obligation.epoch,
        ),
    )
}

pub fn repair_authority_for_identity(
    obligation: &MobMemberTrustWiringObligation,
    identity: &str,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;
    Ok(
        meerkat_core::comms::CommsTrustMutationAuthority::from_generated_mob_machine_peer_repair(
            peer_id.to_owned(),
            obligation.epoch,
        ),
    )
}
