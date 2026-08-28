//! Controller-side routing for source-owned forked-participant capabilities
//! (issue #159, phase 2).
//!
//! This module owns the TRANSPORT shape of the controller's two capability
//! verbs and nothing else. Every semantic decision stays where it already
//! lives: the source-owner [`ForkedParticipantService`] decides local
//! lifecycle legality by driving the canonical lifecycle machine, and the
//! owning member host decides it for a placed source. The controller only
//! resolves WHICH owner serves a request and carries the typed result back.
//!
//! Two routing rules are worth stating explicitly because they are easy to get
//! subtly wrong.
//!
//! - Creation routes by CURRENT source residency. A fork is taken from a live
//!   conversation, so the exact roster member, its exact bridge session, and
//!   (when placed) its exact host incarnation must all be current.
//! - Revocation routes by the CAPABILITY's own immutable owner route, never by
//!   current source residency. The capability outlives its source member: a
//!   retired local source still has a durable local record, and a placed
//!   capability is still owned by the host named in the reference even after
//!   the source member is gone from that host.
//!
//! [`ForkedParticipantService`]: crate::forked_participant::ForkedParticipantService

use std::time::Duration;

use crate::forked_participant::{
    ForkedParticipantAttachmentAssociation, ForkedParticipantOperationScope, ForkedParticipantRef,
    ForkedParticipantRequestId, ForkedParticipantReusePolicy, bridge_ref,
};
use crate::ids::AgentIdentity;
use crate::machines::mob_machine::HostId;

use super::bridge_protocol::{
    BridgeCreateForkedParticipantPayload, BridgeForkedParticipantAttachment,
    BridgeForkedParticipantReuse, BridgeForkedParticipantScope, BridgeMemberIncarnation,
    BridgePeerSpec, BridgeProtocolVersion, BridgeRevokeForkedParticipantPayload,
};

/// Wire protocol version every forked-participant command is pinned to.
///
/// The command family was introduced at V6 and the host serving arms require
/// exactly V6, so the controller pins the same constant rather than sending
/// whatever the supervisor authority happens to carry.
pub(super) const FORKED_PARTICIPANT_PROTOCOL_VERSION: BridgeProtocolVersion =
    BridgeProtocolVersion::V6;

/// Bridge round-trip budget for one capability command.
///
/// A create takes a durable complete-boundary fork on the owning host, so it
/// is deliberately given the same order of budget as other durable host
/// lifecycle verbs rather than a control-plane ping budget.
pub(super) const FORKED_PARTICIPANT_BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);

/// A controller request to create one source-owned forked participant.
///
/// There is no owner-route field: the route is DERIVED from where the source
/// member actually lives, so a caller cannot aim a capability at a realm or
/// host that does not own the source. There is likewise no tool-policy field —
/// the branch inherits the source's effective execution context.
#[derive(Debug, Clone, PartialEq)]
pub struct ForkedParticipantCreateRequest {
    /// Source member whose conversation is forked.
    pub source_identity: AgentIdentity,
    /// Optional exact source generation/profile observation. The serialized
    /// actor validates it immediately before capability creation.
    pub expected_profile: Option<super::handle::MemberExecutionProfileWitness>,
    /// Caller-stable idempotency identity. The same exact request replays.
    pub request_id: ForkedParticipantRequestId,
    /// Complete-boundary prefix length; `None` selects the whole transcript.
    pub prefix_message_count: Option<usize>,
    /// Operations the holder may perform.
    pub scope: ForkedParticipantOperationScope,
    /// One-shot or bounded reuse.
    pub reuse: ForkedParticipantReusePolicy,
    /// Requested time-to-live.
    pub ttl: Duration,
}

pub(super) fn wire_scope(scope: ForkedParticipantOperationScope) -> BridgeForkedParticipantScope {
    match scope {
        ForkedParticipantOperationScope::Invoke => BridgeForkedParticipantScope::Invoke,
        ForkedParticipantOperationScope::Observe => BridgeForkedParticipantScope::Observe,
        ForkedParticipantOperationScope::InvokeAndObserve => {
            BridgeForkedParticipantScope::InvokeAndObserve
        }
    }
}

pub(super) fn wire_reuse(reuse: ForkedParticipantReusePolicy) -> BridgeForkedParticipantReuse {
    match reuse {
        ForkedParticipantReusePolicy::OneShot => BridgeForkedParticipantReuse::OneShot,
        ForkedParticipantReusePolicy::BoundedReuse { max_uses } => {
            BridgeForkedParticipantReuse::BoundedReuse { max_uses }
        }
    }
}

/// Build the V6 create payload for one exact placed source incarnation.
///
/// The TTL is carried in whole milliseconds; a duration that cannot be
/// represented is refused rather than truncated, because the TTL is part of
/// the request fingerprint the owner replays against.
pub(super) fn create_payload(
    supervisor: BridgePeerSpec,
    epoch: u64,
    binding_generation: u64,
    source_member: BridgeMemberIncarnation,
    request: &ForkedParticipantCreateRequest,
) -> Option<BridgeCreateForkedParticipantPayload> {
    Some(BridgeCreateForkedParticipantPayload {
        supervisor,
        epoch,
        binding_generation,
        protocol_version: FORKED_PARTICIPANT_PROTOCOL_VERSION,
        source_member,
        request_id: request.request_id.as_str().to_string(),
        prefix_message_count: request
            .prefix_message_count
            .map(u64::try_from)
            .transpose()
            .ok()?,
        scope: wire_scope(request.scope),
        reuse: wire_reuse(request.reuse),
        ttl_millis: u64::try_from(request.ttl.as_millis()).ok()?,
    })
}

/// Build the V6 revoke payload for one capability.
///
/// `source_member` carries the capability's own STABLE source provenance —
/// the member identity and the source session the fork was taken from — plus
/// the host route and the host's current binding generation, which is what the
/// receiving host admits the command against. It deliberately carries no
/// residency generation/fence claim: revocation must succeed after the source
/// member has retired, and the host's revoke admission validates supervisor
/// authority and the immutable reference rather than current residency.
pub(super) fn revoke_payload(
    supervisor: BridgePeerSpec,
    epoch: u64,
    mob_id: &crate::MobId,
    host_id: &str,
    binding_generation: u64,
    capability: &ForkedParticipantRef,
) -> BridgeRevokeForkedParticipantPayload {
    BridgeRevokeForkedParticipantPayload {
        supervisor,
        epoch,
        binding_generation,
        protocol_version: FORKED_PARTICIPANT_PROTOCOL_VERSION,
        source_member: BridgeMemberIncarnation {
            mob_id: mob_id.to_string(),
            agent_identity: capability.source_identity().as_str().to_string(),
            host_id: host_id.to_string(),
            binding_generation,
            member_session_id: capability.provenance().source_session_id.to_string(),
            generation: 0,
            fence_token: 0,
        },
        capability: crate::forked_participant::bridge_ref(capability),
    }
}

// ===========================================================================
// Capability-aware attached spawn (target-mob side, LOCAL owner route only)
// ===========================================================================

/// Outcome of one capability-aware attached spawn.
///
/// The value is explicit and complete on purpose: a coordinator that seated a
/// capability as an ordinary member needs the spawn result AND the exact lease
/// identity it now owns, because releasing that lease stays its own explicit
/// act. There is no `Drop`-spawned async cleanup anywhere on this path.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AttachedForkedParticipantSpawn {
    /// Ordinary spawn result for the seated member.
    pub spawn: super::handle::SpawnResult,
    /// Full immutable capability reference the member was seated under.
    pub capability: ForkedParticipantRef,
    /// The exact attachment the target mob now holds.
    pub attachment_id: crate::forked_participant::ForkedParticipantAttachmentId,
    /// Who admitted and now owns this attachment's lease.
    pub lease: AttachedForkedParticipantLease,
}

/// Which owner admitted the seated attachment, and what it can honestly say
/// about it.
///
/// A LOCAL seating drives this runtime's own source-owner service, so the
/// caller gets the machine's real typed grant. A HOST seating is admitted by
/// the owning member host inside its ordinary V6 materialization, and the
/// materialize ack carries no grant — so this controller reports the owning
/// host rather than synthesizing a `use_index`/`replayed` verdict it never
/// observed. Fabricating one would be exactly the kind of invented lifecycle
/// truth the capability layer exists to prevent.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AttachedForkedParticipantLease {
    /// Admitted by this runtime's source-owner service.
    Local {
        /// The owner's typed grant. `replayed` distinguishes an exact retry
        /// from a fresh use of the capability's reuse budget.
        grant: crate::forked_participant::ForkedParticipantGrant,
    },
    /// Admitted and owned by the capability's owning member host.
    HostOwned {
        /// Host that owns the capability, its attachment, and its release.
        host_id: HostId,
    },
}

impl AttachedForkedParticipantLease {
    /// The owner's typed grant, when this controller is the owner.
    ///
    /// `None` is a host-owned lease, never a missing local one.
    #[must_use]
    pub fn grant(&self) -> Option<&crate::forked_participant::ForkedParticipantGrant> {
        match self {
            Self::Local { grant } => Some(grant),
            Self::HostOwned { .. } => None,
        }
    }

    /// Host that owns this lease, when the capability is host-owned.
    #[must_use]
    pub fn host_id(&self) -> Option<&HostId> {
        match self {
            Self::Local { .. } => None,
            Self::HostOwned { host_id } => Some(host_id),
        }
    }
}

/// Project one admitted association onto its V6 materialize carrier.
///
/// Metadata only: the full immutable reference plus the attachment identity.
/// No credential material and no transcript body crosses this boundary, and
/// the association is deliberately NOT folded into `PortableMemberSpec` — it
/// is spawn-request authority, not part of the member's portable definition,
/// and it must never reach the spec digest.
pub(super) fn wire_attachment(
    association: &ForkedParticipantAttachmentAssociation,
) -> BridgeForkedParticipantAttachment {
    BridgeForkedParticipantAttachment {
        attachment_id: association.attachment_id.as_str().to_string(),
        capability: bridge_ref(&association.capability),
    }
}

/// Reject any target spec that would widen, re-point, or place the branch.
///
/// Issue #159 requires the fork to inherit the SOURCE's effective tool, auth,
/// realm, and filesystem boundaries. The capability layer therefore never
/// accepts a replacement for any of them: a caller that supplies one is
/// refused rather than silently ignored, because silently ignoring a policy
/// argument is indistinguishable (to the caller) from honouring it.
///
/// Launch mode is owned by the API, not the caller: the only legal launch is a
/// `Resume` of the capability's own exact fork session. `Fresh` (the default)
/// is accepted and rewritten by the caller of this function; an explicitly
/// declared `Resume` must name exactly that session and may not carry a role
/// migration declaration; anything else conflicts and is refused.
pub(super) fn validate_attached_spawn_spec(
    spec: &super::handle::SpawnMemberSpec,
    capability: &ForkedParticipantRef,
    owner_host: Option<&HostId>,
) -> Result<(), crate::MobError> {
    let reject = |detail: String| {
        Err(crate::MobError::ForkedParticipantAttachedSpawnSpecRejected { detail })
    };

    match &spec.launch_mode {
        crate::launch::MemberLaunchMode::Fresh => {}
        crate::launch::MemberLaunchMode::Resume {
            bridge_session_id,
            resume_from_role,
        } => {
            if bridge_session_id != capability.fork_session_id() {
                return reject(
                    "the declared resume session is not this capability's fork session".to_string(),
                );
            }
            if resume_from_role.is_some() {
                return reject(
                    "a capability-aware attached spawn may not also restamp durable member role \
                     identity"
                        .to_string(),
                );
            }
        }
        crate::launch::MemberLaunchMode::Fork { .. } => {
            return reject(
                "a capability-aware attached spawn resumes the capability's own fork session; it \
                 may not declare a second fork"
                    .to_string(),
            );
        }
    }

    // Placement is derived from the capability's OWN owner route, never from
    // the caller. A caller may restate it, but only exactly; anything else is
    // an attempt to seat a branch somewhere its owner does not live.
    match (owner_host, spec.placement.as_ref()) {
        (None, None) => {}
        (None, Some(_)) => {
            return reject(
                "a LOCAL-owned capability may not be seated on a member host".to_string(),
            );
        }
        (Some(owner_host), None) => {
            let _ = owner_host;
        }
        (Some(owner_host), Some(declared)) if declared == owner_host => {}
        (Some(owner_host), Some(declared)) => {
            return reject(format!(
                "this capability is owned by host '{}'; it may not be seated on host '{}'",
                owner_host.as_str(),
                declared.as_str()
            ));
        }
    }
    // Binding/backend describe the target residency's substrate. A LOCAL
    // capability is a controller-local session; a HOST capability's residency
    // is expressed by placement alone, so an explicit substrate declaration is
    // refused rather than reconciled. An unmanaged external runtime is refused
    // on both routes: this mob owns neither its session nor a host that could.
    match (owner_host, spec.binding.as_ref()) {
        (_, None) => {}
        (None, Some(crate::RuntimeBinding::Session)) => {}
        (_, Some(crate::RuntimeBinding::External { .. })) => {
            return reject(
                "an unmanaged external runtime cannot seat a forked-participant capability"
                    .to_string(),
            );
        }
        (None, Some(_)) => {
            return reject(
                "a capability fork session is a controller-local session binding".to_string(),
            );
        }
        (Some(_), Some(_)) => {
            return reject(
                "a host-owned capability's residency is declared by its owner route, not by an \
                 explicit runtime binding"
                    .to_string(),
            );
        }
    }
    match (owner_host, spec.backend) {
        (_, None) => {}
        (None, Some(crate::MobBackendKind::Session)) => {}
        (None, Some(_)) => {
            return reject(
                "a capability fork session is a controller-local session backend".to_string(),
            );
        }
        (Some(_), Some(crate::MobBackendKind::Session)) => {
            return reject(
                "a host-owned capability is materialized on its owning host, not as a \
                 controller-local session"
                    .to_string(),
            );
        }
        (Some(_), Some(_)) => {
            return reject(
                "a host-owned capability's residency is declared by its owner route, not by an \
                 explicit backend"
                    .to_string(),
            );
        }
    }

    if spec.tool_access_policy.is_some() {
        return reject("the branch inherits the source's tool access policy".to_string());
    }
    if spec.tool_dispatch_admission.is_some() {
        return reject("the branch inherits the source's tool dispatch admission".to_string());
    }
    if spec.tool_category_overrides != meerkat_core::ToolCategoryOverrides::default() {
        return reject("the branch inherits the source's tool categories".to_string());
    }
    if spec.inherited_tool_filter.is_some() {
        return reject("the branch inherits the source's tool visibility".to_string());
    }
    if spec.external_tools.is_some() {
        return reject("the branch inherits the source's tool surface".to_string());
    }
    if spec.override_profile.is_some() {
        return reject("the branch inherits the source's resolved profile".to_string());
    }
    if spec.auth_binding.is_some() {
        return reject("the branch inherits the source's auth binding".to_string());
    }
    if spec.system_prompt_override.is_some() {
        return reject("the branch inherits the source's system prompt".to_string());
    }
    if spec.additional_instructions.is_some() {
        return reject("the branch inherits the source's instruction sections".to_string());
    }
    if spec.shell_env.is_some() {
        return reject("the branch inherits the source's shell environment".to_string());
    }
    Ok(())
}
