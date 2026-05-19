// @generated — protocol helpers for `supervisor_trust_revoke`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: RevokeSupervisorTrustEdge
// Closure policy: AckRequired
// Liveness: eventual feedback under comms transport liveness — `send_bridge_response` surfaces the typed outcome

use crate::meerkat_machine::dsl::{MeerkatMachineEffect, PeerId};

#[derive(Debug, Clone)]
pub struct SupervisorTrustRevokeObligation {
    peer_id: String,
    epoch: u64,
}

impl SupervisorTrustRevokeObligation {
    pub fn peer_id(&self) -> &String {
        &self.peer_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

pub fn extract_obligations(
    effects: &[MeerkatMachineEffect],
) -> Vec<SupervisorTrustRevokeObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MeerkatMachineEffect::RevokeSupervisorTrustEdge { peer_id, epoch } => {
                Some(SupervisorTrustRevokeObligation {
                    peer_id: peer_id.clone(),
                    epoch: *epoch,
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
    meerkat_core::comms::CommsTrustMutationAuthority::from_generated_meerkat_machine_supervisor_revoke(
        obligation.peer_id.clone(),
        obligation.epoch,
    )
}
