// @generated — protocol helpers for `mob_external_peer_trust_wiring`
// Composition: meerkat_mob_seam, Producer: mob, Effect: ExternalPeerTrustWiringRequested
// Closure policy: AckRequired
// Liveness: projection mutation is applied by the owning runtime after consuming this typed obligation

use crate::machines::mob_machine::{
    ExternalPeerEdge, MobMachineEffect, MobMachineTransition, PeerId,
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
pub struct MobExternalPeerTrustWiringObligation {
    edge: ExternalPeerEdge,
    local_peer_id: PeerId,
    peer_id: PeerId,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
}

impl MobExternalPeerTrustWiringObligation {
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
    obligation: &MobExternalPeerTrustWiringObligation,
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

impl MobExternalPeerTrustWiringObligation {
    #[allow(unsafe_code)]
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PublicAdd) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
        self.mob_topology_freshness_authority
            .validate_topology_epoch(self.epoch)?;
        if self.peer_id.0 != peer_id {
            return Err(format!(
                "MobMachine external trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
                self.peer_id.0
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
        #[allow(improper_ctypes_definitions, unsafe_code)]
        unsafe extern "Rust" {
            #[link_name = concat!("__meerkat_core_mob_generated_comms_trust_authority_build_v1_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX"))]
            fn core_generated_comms_trust_authority_build(
                token: &'static (dyn std::any::Any + Send + Sync),
                source_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,
                source_epoch: u64,
                trust_row_owner_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,
                operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
                peer_id: String,
                trust_store_peer_id: Option<String>,
                peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
            ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String>;
        }
        match operation {
            Operation::PublicAdd => {
                let peer_descriptor = peer_descriptor.ok_or_else(|| format!("generated comms trust add for peer {peer_id:?} requires a trusted peer descriptor"))?;
                let expected_descriptor = trusted_peer_descriptor_for_request(self, peer_id)?;
                if expected_descriptor != peer_descriptor {
                    return Err(format!(
                        "generated comms trust descriptor for peer {peer_id:?} does not match requested mutation descriptor"
                    ));
                }
                let trust_store_peer_id = self.local_peer_id.0.as_str().to_string();
                let generated_peer_id = peer_descriptor.peer_id.to_string();
                #[allow(unsafe_code)]
                unsafe {
                    core_generated_comms_trust_authority_build(
                        crate::generated_authority_bridge::generated_authority_bridge_token(),
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                        self.epoch,
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
                        generated_peer_id,
                        Some(trust_store_peer_id),
                        Some(peer_descriptor),
                    )
                }
            }
            _ => unreachable!("operation checked above"),
        }
    }
}

pub fn extract_obligations(
    transition: &MobMachineTransition,
) -> Vec<MobExternalPeerTrustWiringObligation> {
    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())
}

pub fn extract_obligations_with_freshness(
    transition: &MobMachineTransition,
    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,
) -> Vec<MobExternalPeerTrustWiringObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MobMachineEffect::ExternalPeerTrustWiringRequested {
                edge,
                local_peer_id,
                peer_id,
                epoch,
            } => Some(MobExternalPeerTrustWiringObligation {
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

pub fn wiring_authority_for_peer(
    obligation: &MobExternalPeerTrustWiringObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MobMachineExternalPeerWiring",
        obligation.peer_id.0.as_str(),
        expected_peer_id,
    )?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,
        expected_peer_id,
        Some(trusted_peer_descriptor_for_request(
            obligation,
            expected_peer_id,
        )?),
    )
}
