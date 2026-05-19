// @generated — protocol helpers for `supervisor_trust_revoke`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: RevokeSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerId};

#[derive(Debug, Clone)]
pub struct SupervisorTrustRevokeObligation {
    peer_id: String,
    epoch: u64,
    comms_trust_authority_claims:
        std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
}

impl SupervisorTrustRevokeObligation {
    pub fn peer_id(&self) -> &String {
        &self.peer_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl meerkat_core::comms::generated_comms_trust_authority::Sealed
    for SupervisorTrustRevokeObligation
{
}
impl meerkat_core::comms::GeneratedCommsTrustAuthoritySource for SupervisorTrustRevokeObligation {
    fn comms_trust_authority_source_kind(
        &self,
    ) -> meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind {
        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineSupervisorRevoke
    }

    fn authorize_comms_trust_authority(
        &self,
        request: &meerkat_core::comms::GeneratedCommsTrustAuthorityRequest<'_>,
    ) -> Result<meerkat_core::comms::GeneratedCommsTrustAuthorityGrant, String> {
        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;
        if !matches!(request.operation(), Operation::PrivateRemove) {
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
) -> Vec<SupervisorTrustRevokeObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::RevokeSupervisorTrustEdge { peer_id, epoch } => {
                Some(SupervisorTrustRevokeObligation {
                    peer_id: peer_id.clone(),
                    epoch: *epoch,
                    comms_trust_authority_claims: Default::default(),
                })
            }
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

pub fn revoke_authority_for_peer(
    obligation: &SupervisorTrustRevokeObligation,
    expected_peer_id: &str,
) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {
    validate_expected_peer(
        "MeerkatMachineSupervisorRevoke",
        &obligation.peer_id,
        expected_peer_id,
    )?;
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_private_remove(
        obligation,
        obligation.peer_id.clone(),
    )
}
