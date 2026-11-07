//! A placed member's operator-instruction target, on its member host.
//!
//! A placed member's session lives on its member host, so its mob records
//! the member's snapshot there, and releases the mob's target when the
//! member retires, over the supervisor bridge (protocol 7):
//! `RecordMemberInstructionTarget` and `ReleaseMemberInstructionTarget`,
//! host-addressed, which the host serves through its own session service
//! without waking the member. Neither waits for a `HostFeatures` probe: a
//! host that does not serve them refuses them typed before any effect
//! (`UnsupportedProtocolVersion` before protocol 7, `Unsupported` for a
//! protocol 7 host from before them), and the member's delivery is then
//! blocked `member_host_unsupported`. MobMachine decides every delivery and
//! release as for a local member; this module only carries them to the host.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::types::SessionId;
use meerkat_core::{InstructionRecordReceipt, InstructionSetErrorCode};

use super::MobSupervisorBridge;
use super::bridge_protocol::{
    BridgeCommand, BridgeInstructionTarget, BridgeInstructionTargetRecordedResponse,
    BridgeInstructionTargetReleasedResponse, BridgeMemberIncarnation, BridgeProtocolVersion,
    BridgeRecordInstructionTargetPayload, BridgeRejectionCause,
    BridgeReleaseInstructionTargetPayload, decode_bridge_payload,
};
use crate::MobError;
use crate::ids::{AgentIdentity, MobId};
use crate::machines::mob_machine as mob_dsl;
use crate::operator_instruction::MemberInstructionDelivery;

/// The bound on one record or release round trip. The member host bounds
/// its own wait for a live member session's turn boundary well inside it,
/// so a write the host gave up on has ended before the next one is sent.
pub(crate) const PLACED_INSTRUCTION_BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// The most bytes an operator-instruction bridge command's JSON may take:
/// the comms frame cap, less a margin for the signed envelope around it
/// (about a kilobyte measured). A larger command can never reach a member
/// host, so it is not sent.
pub(crate) const PLACED_COMMAND_FRAME_BUDGET: usize =
    meerkat_comms::transport::MAX_PAYLOAD_SIZE as usize - 64 * 1024;

/// A placed member's member host: where its session lives.
#[derive(Debug, Clone)]
pub(crate) struct PlacedMemberHost {
    peer: TrustedPeerDescriptor,
    binding_generation: u64,
}

/// A placed member's delivery route: its member host and its exact
/// incarnation there.
#[derive(Debug, Clone)]
pub(crate) struct PlacedMemberInstructionRoute {
    host: PlacedMemberHost,
    member: BridgeMemberIncarnation,
}

/// The member host of a placed member, from the facts MobMachine holds:
/// `None` for a member that is not placed. A placed member whose host has no
/// current binding facts cannot be reached.
pub(crate) fn placed_member_host(
    state: &mob_dsl::MobMachineState,
    agent_identity: &AgentIdentity,
) -> Option<Result<PlacedMemberHost, MobError>> {
    let host = state
        .member_placement
        .get(&mob_dsl::AgentIdentity::from_domain(agent_identity))?;
    Some(member_host_facts(state, agent_identity, host))
}

fn member_host_facts(
    state: &mob_dsl::MobMachineState,
    agent_identity: &AgentIdentity,
    host: &mob_dsl::HostId,
) -> Result<PlacedMemberHost, MobError> {
    let unreachable = |what: &str| {
        MobError::Internal(format!(
            "placed member {agent_identity}'s host '{}' {what}",
            host.as_str()
        ))
    };
    if state.host_bind_phase.get(host) != Some(&mob_dsl::HostBindPhase::Bound) {
        return Err(unreachable("is not bound"));
    }
    let (Some(endpoint), Some(pubkey), Some(binding_generation)) = (
        state.host_endpoints.get(host),
        state.host_public_keys.get(host),
        state.host_binding_generations.get(host).copied(),
    ) else {
        return Err(unreachable("has no recorded binding facts"));
    };
    let peer = TrustedPeerDescriptor::unsigned_with_pubkey(
        host.as_str(),
        host.as_str(),
        pubkey.0,
        endpoint.0.as_str(),
    )
    .map_err(|error| {
        unreachable(&format!(
            "facts do not form a canonical peer descriptor: {error}"
        ))
    })?;
    Ok(PlacedMemberHost {
        peer,
        binding_generation,
    })
}

/// The delivery route of a placed member to `session_id`, the session
/// MobMachine prepared its delivery for: `None` for a member that is not
/// placed.
pub(crate) fn placed_member_instruction_route(
    state: &mob_dsl::MobMachineState,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
    session_id: &SessionId,
) -> Result<Option<PlacedMemberInstructionRoute>, MobError> {
    let Some(host) = placed_member_host(state, agent_identity) else {
        return Ok(None);
    };
    let host = host?;
    let identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
    let missing = |fact: &str| {
        MobError::Internal(format!(
            "placed member {agent_identity} has no MobMachine {fact}"
        ))
    };
    let host_id = state
        .member_placement
        .get(&identity)
        .ok_or_else(|| missing("placement"))?;
    let member = BridgeMemberIncarnation {
        mob_id: mob_id.to_string(),
        agent_identity: agent_identity.to_string(),
        host_id: host_id.as_str().to_string(),
        binding_generation: state
            .current_placed_spawn_host_binding_generations
            .get(&identity)
            .copied()
            .ok_or_else(|| missing("host binding generation"))?,
        member_session_id: session_id.to_string(),
        generation: state
            .identity_runtime_generations
            .get(&identity)
            .map(|generation| generation.0)
            .ok_or_else(|| missing("generation"))?,
        fence_token: state
            .identity_runtime_fence_tokens
            .get(&identity)
            .map(|fence| fence.0)
            .ok_or_else(|| missing("fence token"))?,
    };
    Ok(Some(PlacedMemberInstructionRoute { host, member }))
}

/// Why a placed member's record or release did not land.
#[derive(Debug)]
pub(crate) enum PlacedInstructionFailure {
    /// The member session refused it, typed.
    Refused(InstructionSetErrorCode),
    /// The member host does not serve the command (before protocol 7, or a
    /// protocol 7 host from before it).
    HostUnsupported,
    /// The command would not fit the comms frame, so it was not sent: the
    /// member host can never receive it.
    Oversized { bytes: usize },
    /// The incarnation the command named is not the one the host holds
    /// (record), or a member holds the session again (release).
    Stale,
    /// It did not reach the host, or the host could not serve it now.
    Failed(MobError),
}

impl PlacedInstructionFailure {
    fn from_error(error: MobError) -> Self {
        match error {
            MobError::BridgeCommandRejected { cause, reason } => match cause {
                BridgeRejectionCause::InstructionTargetRefused { code } => Self::Refused(code),
                BridgeRejectionCause::UnsupportedProtocolVersion
                | BridgeRejectionCause::Unsupported => Self::HostUnsupported,
                BridgeRejectionCause::StaleFence => Self::Stale,
                cause => Self::Failed(MobError::BridgeCommandRejected { cause, reason }),
            },
            error => Self::Failed(error),
        }
    }
}

async fn send<R: super::bridge_protocol::FromBridgeReply>(
    bridge: &Arc<MobSupervisorBridge>,
    peer: &TrustedPeerDescriptor,
    command: impl FnOnce(super::bridge_protocol::BridgePeerSpec, u64) -> BridgeCommand,
    context: &str,
) -> Result<R, PlacedInstructionFailure> {
    let authority = bridge.authority().await;
    let supervisor = bridge
        .supervisor_spec_for_authority_and_recipient(&authority, peer)
        .await
        .map_err(PlacedInstructionFailure::Failed)?;
    let command = command(supervisor.into(), authority.epoch);
    // A command past the frame budget can never reach the host (the frame
    // codec refuses it): it is not sent. Its JSON bounds the frame's CBOR
    // body, which carries no string escapes.
    let bytes = serde_json::to_vec(&command)
        .map_err(|error| {
            PlacedInstructionFailure::Failed(MobError::Internal(format!(
                "failed to encode the {context} command: {error}"
            )))
        })?
        .len();
    if bytes > PLACED_COMMAND_FRAME_BUDGET {
        return Err(PlacedInstructionFailure::Oversized { bytes });
    }
    let value = bridge
        .send_bridge_command_as_authority(
            &authority,
            peer,
            &command,
            PLACED_INSTRUCTION_BRIDGE_TIMEOUT,
        )
        .await
        .map_err(PlacedInstructionFailure::Failed)?;
    decode_bridge_payload(&command, value, context).map_err(PlacedInstructionFailure::from_error)
}

/// Record a placed member's snapshot in its session on its member host.
pub(crate) async fn record_placed_member_instruction_target(
    bridge: &Arc<MobSupervisorBridge>,
    route: &PlacedMemberInstructionRoute,
    delivery: &MemberInstructionDelivery,
) -> Result<InstructionRecordReceipt, PlacedInstructionFailure> {
    let target = BridgeInstructionTarget::from_record(delivery.record());
    let recorded: BridgeInstructionTargetRecordedResponse = send(
        bridge,
        &route.host.peer,
        |supervisor, epoch| {
            BridgeCommand::RecordMemberInstructionTarget(BridgeRecordInstructionTargetPayload {
                supervisor,
                epoch,
                binding_generation: route.host.binding_generation,
                protocol_version: BridgeProtocolVersion::V7,
                member: route.member.clone(),
                target,
            })
        },
        "record member instruction target",
    )
    .await?;
    Ok(InstructionRecordReceipt {
        disposition: recorded.disposition,
        target_generation: recorded.target_generation,
    })
}

/// Release a retired placed member's target of `session_id` on its member
/// host, for its mob: `Ok` when the target is released, already unowned, or
/// there is none to release (the session is gone, a member holds it again,
/// the host never recorded one).
pub(crate) async fn release_placed_member_instruction_target(
    bridge: &Arc<MobSupervisorBridge>,
    host: &PlacedMemberHost,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
    session_id: &SessionId,
) -> Result<(), MobError> {
    let released: Result<BridgeInstructionTargetReleasedResponse, _> = send(
        bridge,
        &host.peer,
        |supervisor, epoch| {
            BridgeCommand::ReleaseMemberInstructionTarget(BridgeReleaseInstructionTargetPayload {
                supervisor,
                epoch,
                binding_generation: host.binding_generation,
                protocol_version: BridgeProtocolVersion::V7,
                mob_id: mob_id.to_string(),
                agent_identity: agent_identity.to_string(),
                member_session_id: session_id.to_string(),
            })
        },
        "release member instruction target",
    )
    .await;
    match released {
        Ok(_) => Ok(()),
        // A host that does not serve the command recorded nothing; a member
        // that holds the session again owns its target now; another owner's
        // target, a session that keeps no durable document or one whose
        // instruction state does not read, has no target of this mob's to
        // release.
        Err(
            PlacedInstructionFailure::HostUnsupported
            | PlacedInstructionFailure::Stale
            | PlacedInstructionFailure::Refused(
                InstructionSetErrorCode::OwnedBySession
                | InstructionSetErrorCode::OwnedByOtherMob
                | InstructionSetErrorCode::DurabilityUnavailable
                | InstructionSetErrorCode::MalformedInstructionHistory,
            ),
        ) => Ok(()),
        Err(PlacedInstructionFailure::Refused(code)) => Err(MobError::Internal(format!(
            "the retiring placed member's operator instruction target could not be released: \
             refused {code:?}"
        ))),
        Err(PlacedInstructionFailure::Oversized { bytes }) => Err(MobError::Internal(format!(
            "the release command of a retiring placed member is {bytes} bytes, past the \
             bridge frame budget"
        ))),
        Err(PlacedInstructionFailure::Failed(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(cause: BridgeRejectionCause) -> MobError {
        MobError::BridgeCommandRejected {
            cause,
            reason: "refused".to_string(),
        }
    }

    /// A member host's refusal reads as the failure it is: a host that does
    /// not serve the command (before protocol 7, or a protocol 7 host from
    /// before it), another incarnation on the host, a session's typed
    /// refusal; anything else did not land and is retried.
    #[test]
    fn a_member_hosts_rejection_reads_as_its_failure() {
        for cause in [
            BridgeRejectionCause::UnsupportedProtocolVersion,
            BridgeRejectionCause::Unsupported,
        ] {
            assert!(matches!(
                PlacedInstructionFailure::from_error(rejected(cause)),
                PlacedInstructionFailure::HostUnsupported
            ));
        }
        assert!(matches!(
            PlacedInstructionFailure::from_error(rejected(BridgeRejectionCause::StaleFence)),
            PlacedInstructionFailure::Stale
        ));
        assert!(matches!(
            PlacedInstructionFailure::from_error(rejected(
                BridgeRejectionCause::InstructionTargetRefused {
                    code: InstructionSetErrorCode::OwnedByOtherMob,
                }
            )),
            PlacedInstructionFailure::Refused(InstructionSetErrorCode::OwnedByOtherMob)
        ));
        for error in [
            rejected(BridgeRejectionCause::Unavailable),
            rejected(BridgeRejectionCause::Internal),
            MobError::Internal("the round trip timed out".to_string()),
        ] {
            assert!(matches!(
                PlacedInstructionFailure::from_error(error),
                PlacedInstructionFailure::Failed(_)
            ));
        }
    }
}
