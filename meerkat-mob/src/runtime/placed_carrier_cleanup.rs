//! Shared transport certification for exact placed-carrier cleanup.
//!
//! Both the live actor and cold builder use this lane.  A failed
//! `ReleaseMember` is never classified by error text: authenticated
//! `HostStatus` must prove the exact tuple absent, or a present tuple receives
//! one exact release retry.

use super::bridge_protocol::{
    BridgeCommand, BridgeHostStatusPayload, BridgeHostStatusResponse, BridgeProtocolVersion,
    decode_bridge_payload,
};
use super::provisioner::{HostMemberReleaseRequest, MobProvisioner};
use super::supervisor_bridge::MobSupervisorBridge;
use crate::MobId;
use crate::error::MobError;
use crate::machines::mob_machine::{HostId, MobMachineAuthority};
use crate::store::{MobPlacedSpawnCarrierRecord, PlacedSpawnCarrierPhase};
use meerkat_core::comms::TrustedPeerDescriptor;
use std::sync::Arc;

fn rejection_allows_exact_absence_certification(error: &MobError) -> bool {
    matches!(
        error,
        MobError::BridgeCommandRejected {
            cause: super::bridge_protocol::BridgeRejectionCause::Unsupported,
            ..
        }
    )
}

pub(super) async fn release_placed_attempt_or_certify_absent(
    mob_id: &MobId,
    carrier: &MobPlacedSpawnCarrierRecord,
    authority: &MobMachineAuthority,
    provisioner: &dyn MobProvisioner,
    supervisor_bridge: Arc<MobSupervisorBridge>,
) -> Result<(), MobError> {
    let host_id = HostId::from(carrier.host_id.to_string());
    let state = authority.state();
    let active_binding_generation = state.host_binding_generations.get(&host_id).copied();
    let binding_generation_highwater = state
        .host_binding_generation_highwater
        .get(&host_id)
        .copied()
        .unwrap_or(0);
    if carrier.host_binding_generation == 0 {
        return Err(MobError::Internal(format!(
            "placed carrier '{}' has invalid zero host-binding generation",
            carrier.agent_identity
        )));
    }
    if active_binding_generation != Some(carrier.host_binding_generation) {
        let exact_revoke_confirmed = state.confirmed_host_binding_revocations.contains(
            &crate::machines::mob_machine::HostBindingGenerationTombstone {
                host_id: host_id.clone(),
                binding_generation: carrier.host_binding_generation,
            },
        );
        let carrier_is_superseded = match &carrier.phase {
            PlacedSpawnCarrierPhase::Pending => {
                active_binding_generation
                    .is_some_and(|active| active > carrier.host_binding_generation)
                    || (active_binding_generation.is_none()
                        && binding_generation_highwater >= carrier.host_binding_generation)
                    || exact_revoke_confirmed
            }
            PlacedSpawnCarrierPhase::Committed(_) => {
                exact_revoke_confirmed
                    && state.host_bind_phase.get(&host_id)
                        != Some(&crate::machines::mob_machine::HostBindPhase::Bound)
            }
        };
        if carrier_is_superseded {
            tracing::info!(
                mob_id = %mob_id,
                host = %host_id.as_str(),
                agent_identity = %carrier.agent_identity,
                generation = carrier.generation,
                fence_token = carrier.fence_token,
                carrier_binding_generation = carrier.host_binding_generation,
                active_binding_generation = ?active_binding_generation,
                binding_generation_highwater,
                "certified stale placed carrier terminal without sending under replacement host authority"
            );
            return Ok(());
        }
        return Err(MobError::Internal(format!(
            "placed carrier '{}' binding generation {} conflicts with host '{}' active={active_binding_generation:?} highwater={binding_generation_highwater}",
            carrier.agent_identity,
            carrier.host_binding_generation,
            host_id.as_str()
        )));
    }
    if state.host_bind_phase.get(&host_id)
        != Some(&crate::machines::mob_machine::HostBindPhase::Bound)
    {
        return Err(MobError::Internal(format!(
            "placed carrier host '{}' binding generation {} is not bound",
            host_id.as_str(),
            carrier.host_binding_generation
        )));
    }
    let endpoint = state.host_endpoints.get(&host_id).cloned().ok_or_else(|| {
        MobError::Internal(format!(
            "placed carrier host '{}' has no machine-owned endpoint",
            host_id.as_str()
        ))
    })?;
    let public_key = state
        .host_public_keys
        .get(&host_id)
        .copied()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "placed carrier host '{}' has no machine-owned signing key",
                host_id.as_str()
            ))
        })?;
    let binding_generation = carrier.host_binding_generation;
    let host_peer = TrustedPeerDescriptor::unsigned_with_pubkey(
        host_id.as_str(),
        host_id.as_str(),
        public_key.0,
        endpoint.0.as_str(),
    )
    .map_err(|error| {
        MobError::Internal(format!(
            "placed carrier host '{}' facts are not a canonical peer: {error}",
            host_id.as_str()
        ))
    })?;
    let bridge_authority = supervisor_bridge.authority().await;
    let supervisor = supervisor_bridge
        .supervisor_spec_for_authority_and_recipient(&bridge_authority, &host_peer)
        .await?;
    let release = || HostMemberReleaseRequest {
        mob_id: mob_id.clone(),
        agent_identity: carrier.agent_identity.clone(),
        generation: carrier.generation,
        fence_token: carrier.fence_token,
        supervisor_authority: bridge_authority.clone(),
        supervisor: supervisor.clone().into(),
        binding_generation,
        host: host_peer.clone(),
    };
    let first_error = match provisioner.release_host_member(release()).await {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };

    // UnknownMember is carried by the bridge's coarse `Unsupported` cause.
    // It is a definitive no-effect result, but not a cleanup failure: an
    // authenticated HostStatus under this same binding can certify the exact
    // carrier tuple absent. Other authority/transport rejections remain
    // fail-closed and retain the retiring anchor.
    if !rejection_allows_exact_absence_certification(&first_error)
        && matches!(&first_error, MobError::BridgeCommandRejected { .. })
    {
        return Err(first_error);
    }

    let command = BridgeCommand::HostStatus(BridgeHostStatusPayload {
        supervisor: supervisor.clone().into(),
        epoch: bridge_authority.epoch,
        binding_generation,
        protocol_version: BridgeProtocolVersion::V4,
        mob_id: mob_id.to_string(),
    });
    let value = supervisor_bridge
        .send_bridge_command_as_authority(
            &bridge_authority,
            &host_peer,
            &command,
            std::time::Duration::from_secs(10),
        )
        .await
        .map_err(|status_error| {
            MobError::Internal(format!(
                "ReleaseMember for '{}' tuple ({},{}) failed ({first_error}) and authenticated HostStatus remained unavailable: {status_error}",
                carrier.agent_identity, carrier.generation, carrier.fence_token
            ))
        })?;
    if let Some(rejection) =
        super::bridge_protocol::decode_bridge_rejection_reply(command.protocol_version(), &value)
    {
        return Err(MobError::from(rejection));
    }
    let status: BridgeHostStatusResponse =
        decode_bridge_payload(&command, value, "placed carrier cleanup HostStatus")?;
    let exact_present = status.members.iter().any(|member| {
        member.agent_identity == carrier.agent_identity
            && member.generation == carrier.generation
            && member.fence_token == carrier.fence_token
    });
    if !exact_present {
        tracing::info!(
            mob_id = %mob_id,
            host = %host_id.as_str(),
            agent_identity = %carrier.agent_identity,
            generation = carrier.generation,
            fence_token = carrier.fence_token,
            error = %first_error,
            "authenticated HostStatus certified the uncertain placed tuple absent"
        );
        return Ok(());
    }
    provisioner
        .release_host_member(release())
        .await
        .map(|_| ())
        .map_err(|retry_error| {
            MobError::Internal(format!(
                "ReleaseMember for '{}' tuple ({},{}) failed ({first_error}); HostStatus proved it present and exact retry failed: {retry_error}",
                carrier.agent_identity, carrier.generation, carrier.fence_token
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::rejection_allows_exact_absence_certification;
    use crate::MobError;
    use crate::runtime::bridge_protocol::BridgeRejectionCause;

    #[test]
    fn only_unknown_member_carrier_advances_to_host_status_certification() {
        let rejected = |cause| MobError::BridgeCommandRejected {
            cause,
            reason: "test rejection".to_string(),
        };
        assert!(rejection_allows_exact_absence_certification(&rejected(
            BridgeRejectionCause::Unsupported,
        )));
        assert!(!rejection_allows_exact_absence_certification(&rejected(
            BridgeRejectionCause::StaleFence,
        )));
        assert!(!rejection_allows_exact_absence_certification(&rejected(
            BridgeRejectionCause::NotBound,
        )));
    }
}
