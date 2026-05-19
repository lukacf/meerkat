// @generated — protocol helpers for `supervisor_trust_publish`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: PublishSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerId};

#[derive(Debug, Clone)]
pub struct SupervisorTrustPublishObligation {
    peer_id: String,
    name: String,
    address: String,
    signing_public_key: Option<String>,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl SupervisorTrustPublishObligation {
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

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for SupervisorTrustPublishObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource for SupervisorTrustPublishObligation {
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish
    }

    fn authorize_comms_trust_authority(
        &self,
        request: &meerkat_core::comms::GeneratedCommsTrustAuthorityRequest<'_>,
    ) -> Result<meerkat_core::comms::GeneratedCommsTrustAuthorityGrant, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(request.operation(), Operation::PrivateAdd) {
            return Err(format!(
                "generated comms trust source {:?} cannot authorize operation {:?}",
                self.comms_trust_authority_source_kind(),
                request.operation()
            ));
        }
        if self.peer_id != request.peer_id() {
            return Err(format!(
                "MeerkatMachine supervisor trust obligation peer_id {:?} does not match requested peer {:?}",
                self.peer_id,
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
            return meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new_add(request, self.epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish, peer_descriptor);
        }
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(request, self.epoch, meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish))
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
                peer_id,
                name,
                address,
                signing_public_key,
                epoch,
            } => Some(SupervisorTrustPublishObligation {
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
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_private_add(
        obligation,
        trusted_peer_descriptor_for_request(obligation, expected_peer_id)?,
    )
}
