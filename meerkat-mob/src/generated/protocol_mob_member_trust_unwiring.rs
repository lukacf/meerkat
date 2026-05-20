// @generated — protocol helpers for `mob_member_trust_unwiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: MemberTrustUnwiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId, WiringEdge,
};

#[derive(Debug, Clone)]
pub struct MobTopologyFreshnessAuthority {
    topology_epoch: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

impl MobTopologyFreshnessAuthority {
    pub fn from_authority(authority: &crate::machines::mob_machine::MobMachineAuthority) -> Self {
        Self::from_live_topology_epoch(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
            authority.state().topology_epoch,
        )))
    }

    pub fn from_live_topology_epoch(
        topology_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        Self {
            topology_epoch: Some(topology_epoch),
        }
    }

    fn missing() -> Self {
        Self {
            topology_epoch: None,
        }
    }

    fn validate_topology_epoch(&self, expected_epoch: u64) -> Result<(), String> {
        let Some(topology_epoch) = &self.topology_epoch else {
            return Err("generated MobMachine topology freshness authority is absent".to_string());
        };
        let current_epoch = topology_epoch.load(std::sync::atomic::Ordering::Acquire);
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
pub struct MobMemberTrustUnwiringObligation {
    edge: WiringEdge,
    a_peer_id: PeerId,
    b_peer_id: PeerId,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
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

impl MobMemberTrustUnwiringObligation {
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PublicRemove) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
        self.mob_topology_freshness_authority
            .validate_topology_epoch(self.epoch)?;
        if self.a_peer_id.0 != peer_id && self.b_peer_id.0 != peer_id {
            return Err(format!(
                "MobMachine member trust obligation does not carry requested peer {peer_id:?}"
            ));
        }
        let claim_key = format!("{operation:?}:{peer_id}");
        let mut claims = self.comms_trust_authority_claims.lock().map_err(|_| {
            "generated comms trust authority source claims were poisoned".to_string()
        })?;
        if !claims.insert(claim_key) {
            return Err(format!(
                "generated comms trust authority source already minted {operation:?} for peer {peer_id:?}"
            ));
        }
        match operation {
            Operation::PublicRemove => {
                if peer_descriptor.is_some() {
                    return Err(format!(
                        "generated comms trust remove for peer {peer_id:?} must not carry a trusted peer descriptor"
                    ));
                }
                meerkat_core::generated::comms_trust_authority_sources::mob_member_trust_unwiring_public_remove(self.epoch, if self.a_peer_id.0 == peer_id { self.b_peer_id.0.as_str() } else if self.b_peer_id.0 == peer_id { self.a_peer_id.0.as_str() } else { return Err(format!("MobMachine member trust obligation does not carry requested peer {peer_id:?}")); }, peer_id)
            }
            _ => unreachable!("operation checked above"),
        }
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobMemberTrustUnwiringObligation> {
    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MobMachineTransition,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
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
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicRemove,
        peer_id,
        None,
    )
}
