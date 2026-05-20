// @generated — protocol helpers for `supervisor_trust_publish`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: PublishSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};

#[derive(Debug, Clone)]
pub struct SupervisorTrustPublishObligation {
    local_endpoint: Option<PeerEndpoint>,
    peer_id: String,
    name: String,
    address: String,
    signing_public_key: Option<String>,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl SupervisorTrustPublishObligation {
    pub fn local_endpoint(&self) -> &Option<PeerEndpoint> {
        &self.local_endpoint
    }

    pub fn peer_id(&self) -> &String {
        &self.peer_id
    }

    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn address(&self) -> &String {
        &self.address
    }

    pub fn signing_public_key(&self) -> &Option<String> {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

fn trusted_peer_descriptor_for_request(
    obligation: &SupervisorTrustPublishObligation,
    peer_id: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {
    if obligation.peer_id != peer_id {
        return Err(format!(
            "MeerkatMachine supervisor trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
            obligation.peer_id
        ));
    }
    let signing_public_key = obligation.signing_public_key.as_ref().ok_or_else(|| {
        "generated supervisor trust publish obligation omitted signing public key".to_string()
    })?;
    let pubkey = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)?;
    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
        obligation.name.clone(),
        obligation.peer_id.clone(),
        pubkey,
        obligation.address.clone(),
    )
}

impl SupervisorTrustPublishObligation {
    fn authorize_comms_trust_authority(
        &self,
        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,
        peer_id: &str,
        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,
    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(operation, Operation::PrivateAdd | Operation::PrivateRemove) {
            return Err(format!(
                "generated comms trust source cannot authorize operation {operation:?}"
            ));
        }
        if self.peer_id != peer_id {
            return Err(format!(
                "MeerkatMachine supervisor trust obligation peer_id {:?} does not match requested peer {peer_id:?}",
                self.peer_id
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
            Operation::PrivateAdd => {
                let peer_descriptor = peer_descriptor.ok_or_else(|| format!("generated comms trust add for peer {peer_id:?} requires a trusted peer descriptor"))?;
                let expected_descriptor = trusted_peer_descriptor_for_request(self, peer_id)?;
                if expected_descriptor != peer_descriptor {
                    return Err(format!(
                        "generated comms trust descriptor for peer {peer_id:?} does not match requested mutation descriptor"
                    ));
                }
                meerkat_core::generated::comms_trust_authority_sources::supervisor_trust_publish_private_add(self.epoch, self.local_endpoint.as_ref().ok_or_else(|| "generated MeerkatMachine trust obligation did not carry local trust-store endpoint".to_string())?.peer_id.0.as_str(), peer_descriptor)
            }
            Operation::PrivateRemove => {
                if peer_descriptor.is_some() {
                    return Err(format!(
                        "generated comms trust remove for peer {peer_id:?} must not carry a trusted peer descriptor"
                    ));
                }
                meerkat_core::generated::comms_trust_authority_sources::supervisor_trust_publish_private_remove(self.epoch, self.local_endpoint.as_ref().ok_or_else(|| "generated MeerkatMachine trust obligation did not carry local trust-store endpoint".to_string())?.peer_id.0.as_str(), peer_id)
            }
            _ => unreachable!("operation checked above"),
        }
    }
}

pub fn extract_obligations(
    transition: &MeerkatMachineTransition,
) -> Vec<SupervisorTrustPublishObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::PublishSupervisorTrustEdge {
                local_endpoint,
                peer_id,
                name,
                address,
                signing_public_key,
                epoch,
            } => Some(SupervisorTrustPublishObligation {
                local_endpoint: local_endpoint.clone(),
                peer_id: peer_id.clone(),
                name: name.clone(),
                address: address.clone(),
                signing_public_key: signing_public_key.clone(),
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

pub fn publish_authority_for_peer(
    obligation: &SupervisorTrustPublishObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MeerkatMachineSupervisorPublish",
        &obligation.peer_id,
        expected_peer_id,
    )?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateAdd,
        expected_peer_id,
        Some(trusted_peer_descriptor_for_request(
            obligation,
            expected_peer_id,
        )?),
    )
}

pub fn cleanup_authority_for_peer(
    obligation: &SupervisorTrustPublishObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MeerkatMachineSupervisorPublishCleanup",
        &obligation.peer_id,
        expected_peer_id,
    )?;
    obligation.authorize_comms_trust_authority(
        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateRemove,
        obligation.peer_id.as_str(),
        None,
    )
}
