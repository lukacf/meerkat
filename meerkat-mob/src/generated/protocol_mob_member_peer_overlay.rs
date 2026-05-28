// @generated — protocol helpers for `mob_member_peer_overlay`
// Composition: meerkat_mob_seam, Producer: mob, Effect: MemberPeerOverlayAuthorized
// Closure policy: PublicationOnly
// Liveness: generated MobMachine peer overlay publication is consumed by the bridge owner before MeerkatMachine peer projection reconciliation

use crate::machines::mob_machine::{
    AgentIdentity, MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId,
};

#[derive(Debug, Clone)]
pub struct MobTopologyFreshnessAuthority {
    topology_epoch: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl MobTopologyFreshnessAuthority {
    pub(crate) fn from_live_topology_epoch(
        topology_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
        source_owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self {
            topology_epoch: Some(topology_epoch),
            source_owner_token: Some(source_owner_token),
        }
    }

    fn missing() -> Self {
        Self {
            topology_epoch: None,
            source_owner_token: None,
        }
    }

    fn source_owner_token(&self) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {
        self.source_owner_token.as_ref().map(std::sync::Arc::clone)
    }

    fn validate_topology_epoch(
        &self,
        expected_epoch: u64,
        allow_next_epoch: bool,
    ) -> Result<(), String> {
        let Some(topology_epoch) = &self.topology_epoch else {
            return Err("generated MobMachine topology freshness authority is absent".to_string());
        };
        let current_epoch = topology_epoch.load(std::sync::atomic::Ordering::Acquire);
        let matches_current = current_epoch == expected_epoch;
        let matches_next = allow_next_epoch && current_epoch.checked_add(1) == Some(expected_epoch);
        if matches_current || matches_next {
            Ok(())
        } else {
            Err(format!(
                "stale generated MobMachine trust obligation at epoch {expected_epoch} (current {current_epoch})"
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct MobMemberPeerOverlayObligation {
    agent_identity: AgentIdentity,
    peer_id: PeerId,
    peer_overlay_endpoints: std::collections::BTreeSet<MemberPeerEndpoint>,
    epoch: u64,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
}

impl MobMemberPeerOverlayObligation {
    pub fn agent_identity(&self) -> &AgentIdentity {
        &self.agent_identity
    }

    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    pub fn peer_overlay_endpoints(&self) -> &std::collections::BTreeSet<MemberPeerEndpoint> {
        &self.peer_overlay_endpoints
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobMemberPeerOverlayObligation> {
    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MobMachineTransition,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
) -> Vec<MobMemberPeerOverlayObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::MemberPeerOverlayAuthorized {
                agent_identity,
                peer_id,
                peer_overlay_endpoints,
                epoch,
            } => Some(MobMemberPeerOverlayObligation {
                agent_identity: agent_identity.clone(),
                peer_id: peer_id.clone(),
                peer_overlay_endpoints: peer_overlay_endpoints.clone(),
                epoch: *epoch,
                mob_topology_freshness_authority: mob_topology_freshness_authority.clone(),
            }),
            _ => None,
        })
        .collect()
}

pub fn validate_overlay_freshness(
    obligation: &MobMemberPeerOverlayObligation,
) -> Result<(), String> {
    obligation
        .mob_topology_freshness_authority
        .validate_topology_epoch(obligation.epoch, false)
}
