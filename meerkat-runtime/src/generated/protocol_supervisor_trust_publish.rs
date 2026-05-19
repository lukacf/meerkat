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
        Ok(meerkat_core::comms::GeneratedCommsTrustAuthorityGrant::new(
            request, self.epoch,
        ))
    }
}

pub fn extract_obligations(
    transition: &MeerkatMachineTransition,
) -> Vec<SupervisorTrustPublishObligation> {
    transition
        .effects
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
        obligation.peer_id.clone(),
    )
}
