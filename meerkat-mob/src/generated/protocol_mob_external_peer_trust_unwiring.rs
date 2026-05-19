// @generated — protocol helpers for `mob_external_peer_trust_unwiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerTrustUnwiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    ExternalPeerEdge, MobMachineEffect, MobMachineTransition, PeerId,
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
pub struct MobExternalPeerTrustUnwiringObligation {
    edge: ExternalPeerEdge,
    local_peer_id: PeerId,
    peer_id: PeerId,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
}

impl MobExternalPeerTrustUnwiringObligation {
    pub fn edge(&self) -> &ExternalPeerEdge {
        &self.edge
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for MobExternalPeerTrustUnwiringObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource
    for MobExternalPeerTrustUnwiringObligation
{
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustUnwiring
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
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(request, self.epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring).with_trust_store_peer_id(self.local_peer_id.0.as_str()))
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobExternalPeerTrustUnwiringObligation> {
    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MobMachineTransition,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
) -> Vec<MobExternalPeerTrustUnwiringObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerTrustUnwiringRequested {
                edge,
                local_peer_id,
                peer_id,
                epoch,
            } => Some(MobExternalPeerTrustUnwiringObligation {
                edge: edge.clone(),
                local_peer_id: local_peer_id.clone(),
                peer_id: peer_id.clone(),
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

pub fn unwiring_authority_for_peer(
    obligation: &MobExternalPeerTrustUnwiringObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MobMachineExternalPeerUnwiring",
        obligation.peer_id.0.as_str(),
        expected_peer_id,
    )?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_public_remove(
        obligation,
        obligation.peer_id.0.clone(),
    )
}
