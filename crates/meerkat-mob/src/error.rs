//! Error types for mob operations.

use crate::control_policy::ScopeDenial;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, FlowId, LoopId, ProfileName, WorkRef};
use crate::runtime::MobState;
use crate::store::FrameAtomicOperation;
use crate::validate::Diagnostic;
use crate::{MobId, RunId, StepId};
use meerkat_contracts::wire::supervisor_bridge::{BridgeRejectionCause, BridgeRejectionReply};
use meerkat_contracts::wire::{
    WireHostUnavailableDetail, WireMobErrorDetail, WireScopeDeniedDetail, WireStaleCursorDetail,
    WireStaleDeliveryScopeDetail, WireStaleFenceDetail,
};
use std::sync::Arc;

/// Runtime capability required from a seated mob member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobMemberCapability {
    /// Interaction-scoped injection used for autonomous console/RPC/flow turns.
    InteractionEventInjector,
    /// An outbound comms runtime the host's content-taint declaration can
    /// install on (session-backed members hold one; external members relay
    /// over the supervisor bridge instead).
    OutboundCommsRuntime,
    /// Runtime-owned live session LLM reconfiguration for exact-turn model,
    /// provider, provider-parameter, and auth-binding overrides.
    SessionLlmReconfigure,
}

/// Typed MobMachine routed-effect kind whose runtime consumer refused the
/// generated composition input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeEffectKind {
    RuntimeBinding,
    RuntimeIngress,
    RuntimeRetire,
}

impl std::fmt::Display for RuntimeEffectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuntimeBinding => f.write_str("runtime_binding"),
            Self::RuntimeIngress => f.write_str("runtime_ingress"),
            Self::RuntimeRetire => f.write_str("runtime_retire"),
        }
    }
}

/// Why a source member cannot serve one forked-participant capability
/// operation.
///
/// These are controller-side ROUTING refusals decided before any capability
/// record is touched: the source-owner service and the owning host remain the
/// only deciders of capability lifecycle legality, so nothing here duplicates
/// a machine verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkedParticipantSourceRejection {
    /// The member holds no canonical bridge session that could be forked.
    NoSourceSession,
    /// The member is an unmanaged external peer runtime: this controller owns
    /// neither its session nor a host route that could own the fork.
    UnmanagedExternal,
    /// The member's canonical residency facts do not currently form a usable
    /// source (stale roster route, retired/destroyed runtime, dormant or
    /// revoked host binding).
    SourceRuntimeBroken,
    /// The source is mid-run. A fork is only defined at a complete boundary.
    SourceBusy,
    /// This runtime composes no forked-participant capability store, so it
    /// cannot own a local capability.
    CapabilityStoreAbsent,
    /// This runtime's session service exposes no capability source runtime.
    SourceRuntimeAbsent,
    /// The bound member host has not negotiated the V6 capability protocol.
    HostProtocolUnsupported,
}

/// Explicit lease verb that cannot be served across a host route on this
/// surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkedParticipantLeaseOperation {
    /// Attach a capability to a temporary coordinator.
    Attach,
    /// Release an attachment from a temporary coordinator.
    Release,
    /// Seat a capability as an ordinary target-mob member.
    AttachedSpawn,
}

impl std::fmt::Display for ForkedParticipantLeaseOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Attach => f.write_str("attach"),
            Self::Release => f.write_str("release"),
            Self::AttachedSpawn => f.write_str("attached spawn"),
        }
    }
}

/// Why a capability's owning member host cannot seat it in this mob right now.
///
/// Every variant is decided from MACHINE-recorded host facts before any bridge
/// traffic: an unbound or under-versioned host must never learn that a
/// capability exists by receiving a command it cannot serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ForkedParticipantOwnerHostRejection {
    /// The capability's owner host is not currently bound in this mob.
    HostNotBound,
    /// The bound host has not negotiated the V6 capability protocol.
    ProtocolUnsupported,
    /// The host is bound but its recorded route facts do not form a peer.
    RouteUnusable,
}

impl std::fmt::Display for ForkedParticipantOwnerHostRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostNotBound => f.write_str("host is not bound in this mob"),
            Self::ProtocolUnsupported => {
                f.write_str("host does not negotiate supervisor bridge V6")
            }
            Self::RouteUnusable => f.write_str("host binding facts do not form a usable route"),
        }
    }
}

impl std::fmt::Display for ForkedParticipantSourceRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSourceSession => f.write_str("source member has no canonical bridge session"),
            Self::UnmanagedExternal => {
                f.write_str("source member is an unmanaged external peer runtime")
            }
            Self::SourceRuntimeBroken => {
                f.write_str("source member residency is not currently usable")
            }
            Self::SourceBusy => f.write_str("source member is running"),
            Self::CapabilityStoreAbsent => {
                f.write_str("this runtime composes no forked-participant capability store")
            }
            Self::SourceRuntimeAbsent => {
                f.write_str("this runtime exposes no forked-participant source runtime")
            }
            Self::HostProtocolUnsupported => {
                f.write_str("bound member host does not negotiate supervisor bridge V6")
            }
        }
    }
}

/// Mob-owned classification of why a mob operation failed.
///
/// This is the typed owner of "what class of failure does this `MobError`
/// represent" — the knowledge of which variants are missing-target vs busy vs
/// transport vs internal lives next to the variants themselves, not in
/// downstream classifiers that re-`match` the enum.
///
/// Consumers (e.g. the schedule delivery host) map this onto their own
/// domain-failure vocabulary (`DeliveryFailureReason`); the mapping lives at
/// the consumer because the schedule-failure type is owned by a crate
/// `meerkat-mob` does not (and must not) depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobFailureClass {
    /// The addressed mob/profile/member/flow/run/work does not exist.
    TargetMissing,
    /// The addressed durable session EXISTS and is intact, but is archived
    /// and its lifecycle refuses revival. Deliberately distinct from
    /// `TargetMissing`: reporting an intact archived transcript as absent
    /// sent operators hunting for data that was on disk all along.
    TargetArchived,
    /// The target exists but cannot accept the operation right now
    /// (e.g. a member id collision).
    TargetBusy,
    /// A transport/persistence/session/timeout fault prevented delivery.
    Transport,
    /// A runtime accepted the work but parked at an external boundary
    /// (callback pending) rather than completing, OR the runtime refuses
    /// further ordinary work until an explicit, named repair action closes
    /// the gap (e.g. `reload_member_registration`, `rotate_supervisor`) —
    /// never resolved by plain busy/backoff retry.
    RuntimeRejected,
    /// An internal/unexpected-state fault.
    Internal,
    /// The mob authority rejected the operation on its own terms.
    MobRejected,
}

/// Typed reason a durable definition authority and its spec projection cannot converge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobDefinitionProjectionMismatchKind {
    /// The projection claims a revision newer than the event-log authority.
    ProjectionAhead,
    /// Authority and projection claim the same epoch but carry different definitions.
    DefinitionMismatch,
}

impl std::fmt::Display for MobDefinitionProjectionMismatchKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProjectionAhead => f.write_str("projection_ahead"),
            Self::DefinitionMismatch => f.write_str("definition_mismatch"),
        }
    }
}

/// How a mob store's creation record and its event log disagree (see
/// [`MobError::IncarnationInconsistent`]). The creation record names the
/// incarnation the store's mob was created at; one record goes with each
/// `MobCreated`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncarnationInconsistency {
    /// A creation record that is not finalized, in a store whose log holds
    /// no `MobCreated`.
    RecordWithoutCreation,
    /// The creation record names another mob than the store's `MobCreated`.
    RecordOfAnotherMob { record_mob_id: MobId },
    /// A finalized creation record (a finished destroy) next to a
    /// `MobCreated` that no creation record names.
    FinalizedRecordWithCreation,
    /// More than one creation record in one store.
    MultipleRecords { count: u64 },
    /// A raw `MobCreated` append over a store that holds a creation record:
    /// only the creation itself writes `MobCreated` with its record. Nothing
    /// was written.
    RawCreationOverRecord,
}

impl std::fmt::Display for IncarnationInconsistency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecordWithoutCreation => {
                f.write_str("its creation record has no MobCreated in the log")
            }
            Self::RecordOfAnotherMob { record_mob_id } => {
                write!(f, "its creation record names mob '{record_mob_id}'")
            }
            Self::FinalizedRecordWithCreation => f.write_str(
                "its creation record is finalized but the log holds a MobCreated it does not name",
            ),
            Self::MultipleRecords { count } => write!(f, "it holds {count} creation records"),
            Self::RawCreationOverRecord => {
                f.write_str("a raw MobCreated append would bypass its creation record")
            }
        }
    }
}

impl std::fmt::Display for MobMemberCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InteractionEventInjector => f.write_str("interaction_event_injector"),
            Self::OutboundCommsRuntime => f.write_str("outbound_comms_runtime"),
            Self::SessionLlmReconfigure => f.write_str("session_llm_reconfigure"),
        }
    }
}

/// Why a durable session cannot serve a resume. Typed companion of
/// [`MobError::SessionUnavailableForResume`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionResumeUnavailableReason {
    /// The document exists and is intact, but is archived and its runtime
    /// lifecycle refuses revival.
    ArchivedNotRevivable,
    /// Durable copies exist, but the session owner cannot prove which
    /// committed boundary is authoritative. The copies remain intact and the
    /// resume is parked until authority can be reconciled.
    CommittedBoundaryUnprovable,
    /// The store-issued authority or lifecycle observation changed while the
    /// body was loaded. No body from that interval is authorized for resume.
    AuthorityChangedDuringMaterialization,
    /// Positive durable authority or lifecycle rows exist without a
    /// materializable session body.
    ContradictoryDurableAuthority,
    /// No durable session exists for this id.
    Absent,
}

impl std::fmt::Display for SessionResumeUnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArchivedNotRevivable => {
                f.write_str("archived and not revivable; the transcript is intact and preserved")
            }
            Self::CommittedBoundaryUnprovable => f.write_str(
                "committed boundary is unprovable; durable copies are retained for reconciliation",
            ),
            Self::AuthorityChangedDuringMaterialization => {
                f.write_str("durable authority changed during resume materialization")
            }
            Self::ContradictoryDurableAuthority => {
                f.write_str("durable authority contradicts the materializable session body")
            }
            Self::Absent => f.write_str("missing durable session snapshot"),
        }
    }
}

/// Typed cause for a member provision that cannot recover inside the current
/// process incarnation.
///
/// The transport owner must construct this cause at the boundary where it
/// observes terminal closure. Callers must never infer it from display prose:
/// ordinary session, provider, or tool failures remain retryable unless they
/// arrive through this explicit carrier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MemberProvisionFailureCause {
    /// The process-local callback transport is terminally closed. A fresh
    /// declaration may install a replacement transport; otherwise recovery
    /// requires a new process incarnation.
    #[error("callback transport closed: {detail}")]
    CallbackTransportClosed { detail: String },
}

/// Why a member's runtime registration holds no committed attachment,
/// projected from the runtime's typed attachment fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRuntimeDetachment {
    /// The session has no runtime registration.
    Unregistered,
    /// The registration lost durable authority and has no attachment.
    ReloadRequired,
    /// The registration is detached and no unregister has started.
    UnregisterNotStarted,
    /// The registration is detached while its unregister is in flight.
    UnregisterInFlight,
    /// The registration's teardown unregister failed and is retained with no
    /// owner until an explicit resume retries it.
    UnregisterFailed,
}

impl MemberRuntimeDetachment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unregistered => "unregistered",
            Self::ReloadRequired => "reload_required",
            Self::UnregisterNotStarted => "unregister_not_started",
            Self::UnregisterInFlight => "unregister_in_flight",
            Self::UnregisterFailed => "unregister_failed",
        }
    }
}

impl std::fmt::Display for MemberRuntimeDetachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The member registration hook call that refused a member's registration.
/// A restored member its host refused stays Broken with this call recorded
/// (see [`MobError::MemberRegistrationRefused`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberRegistrationHookCall {
    /// The hook's `select_destination`.
    SelectDestination,
    /// The hook's `select_agent_mandate`.
    SelectAgentMandate,
    /// The hook's `prepare_member_surface`.
    PrepareMemberSurface,
    /// The hook's `verify_member_surface`.
    VerifyMemberSurface,
}

impl MemberRegistrationHookCall {
    const ALL: [Self; 4] = [
        Self::SelectDestination,
        Self::SelectAgentMandate,
        Self::PrepareMemberSurface,
        Self::VerifyMemberSurface,
    ];

    /// The hook method's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelectDestination => "select_destination",
            Self::SelectAgentMandate => "select_agent_mandate",
            Self::PrepareMemberSurface => "prepare_member_surface",
            Self::VerifyMemberSurface => "verify_member_surface",
        }
    }

    /// The stable code MobMachine records for a restored member this call
    /// refused.
    pub(crate) fn as_code(self) -> &'static str {
        match self {
            Self::SelectDestination => "host_refused_registration.select_destination",
            Self::SelectAgentMandate => "host_refused_registration.select_agent_mandate",
            Self::PrepareMemberSurface => "host_refused_registration.prepare_member_surface",
            Self::VerifyMemberSurface => "host_refused_registration.verify_member_surface",
        }
    }

    /// The call a recorded restore-failure code names; `None` for every
    /// other code. The only reader of these codes.
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|call| call.as_code() == code)
    }
}

impl std::fmt::Display for MemberRegistrationHookCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why role-targets admission refused a flow run.
///
/// The verdict is computed once, inside the mob actor command that admits the
/// run, from MobMachine's own member facts, before anything is written,
/// provisioned or dispatched. On the wire it rides the error data as a closed
/// vocabulary, `{"kind": "<variant>", ...fields}`, so no caller reads the
/// message text to tell a binding refusal from other invalid params.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FlowRoleTargetRefusal {
    /// A bound role is not the role of any step of the flow.
    RoleNotInFlow { role: ProfileName },
    /// A role that a step of the flow dispatches to is not bound.
    RoleUnbound { role: ProfileName },
    /// A quorum step needs more producers than its role's bound set holds.
    QuorumExceedsBoundSet {
        step_id: StepId,
        required: u8,
        bound: usize,
    },
    /// A bound identity is not a member of this mob.
    NotAMember {
        role: ProfileName,
        member: AgentIdentity,
    },
    /// A bound member holds a different role.
    RoleMismatch {
        role: ProfileName,
        member: AgentIdentity,
        member_role: ProfileName,
    },
    /// A bound member holds the role but is not Active.
    NotActive {
        role: ProfileName,
        member: AgentIdentity,
        status: crate::runtime::MobMemberStatus,
    },
}

impl std::fmt::Display for FlowRoleTargetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoleNotInFlow { role } => {
                write!(f, "role '{role}' is not the role of any step of the flow")
            }
            Self::RoleUnbound { role } => {
                write!(f, "role '{role}' is used by the flow but not bound")
            }
            Self::QuorumExceedsBoundSet {
                step_id,
                required,
                bound,
            } => write!(
                f,
                "step '{step_id}' needs a quorum of {required} but its role binds {bound} members"
            ),
            Self::NotAMember { role, member } => {
                write!(
                    f,
                    "member '{member}' bound to role '{role}' is not a member of this mob"
                )
            }
            Self::RoleMismatch {
                role,
                member,
                member_role,
            } => write!(
                f,
                "member '{member}' bound to role '{role}' holds role '{member_role}'"
            ),
            Self::NotActive {
                role,
                member,
                status,
            } => write!(
                f,
                "member '{member}' bound to role '{role}' is not active (status {status:?})"
            ),
        }
    }
}

/// Errors returned by mob operations.
///
/// Non-exhaustive: a later release can add a refusal without breaking a
/// downstream match, which needs a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MobError {
    /// The requested mob does not exist in the runtime/store.
    #[error("mob not found: {0}")]
    MobNotFound(MobId),

    /// The requested profile does not exist in the mob definition.
    #[error("profile not found: {0}")]
    ProfileNotFound(ProfileName),

    /// The requested mob member does not exist in the roster.
    #[error("mob member not found: {0}")]
    MemberNotFound(AgentIdentity),

    /// The member's authorized session has no live actor in the session
    /// service: it is not materialized yet, or its actor was already
    /// discarded. Hosts can retry once the member materializes.
    #[error("mob member '{agent_identity}' has no live session actor for session {session_id}")]
    MemberSessionNotLive {
        agent_identity: AgentIdentity,
        session_id: meerkat_core::SessionId,
    },

    /// An agent event subscription's replay cursor was rejected.
    #[error("agent event cursor {cursor:?} for mob member '{agent_identity}' rejected: {reason}")]
    AgentEventCursorRejected {
        agent_identity: AgentIdentity,
        cursor: meerkat_core::comms::SessionEventCursor,
        reason: meerkat_core::comms::SessionEventCursorRejection,
    },

    /// A stable delivery key already owns different or unprovable input
    /// semantics. Reusing the key cannot reclassify old work as human input.
    #[error("work delivery conflicts with input {input_id} in session {session_id}")]
    WorkInputIdempotencyConflict {
        session_id: meerkat_core::SessionId,
        input_id: meerkat_core::InputId,
    },

    #[error("exact work completion is unavailable for input {input_id} in session {session_id}")]
    WorkInputCompletionUnavailable {
        session_id: meerkat_core::SessionId,
        input_id: meerkat_core::InputId,
    },

    /// The member session's native admission refused or could not decide an
    /// input. `error` is the runtime's own verdict, unchanged.
    #[error("member session {session_id} did not admit the input: {error}")]
    MemberInputRefused {
        session_id: meerkat_core::SessionId,
        error: meerkat_runtime::RuntimeDriverError,
    },

    /// A governed submission must carry its caller-bound authority
    /// association and trusted ingress observation; the mob never supplies
    /// or substitutes either.
    #[error(
        "governed input for mob member '{member_id}' is not bound to an authority association and ingress observation"
    )]
    GovernedInputUnbound { member_id: AgentIdentity },

    /// A durable session could not serve a resume — typed so surfaces can
    /// distinguish archived-but-intact from genuinely absent instead of
    /// parsing prose out of an internal error.
    #[error(
        "session '{session_id}' unavailable for resume: {reason} (runtime state {})",
        runtime_state.as_deref().unwrap_or("<no runtime record>")
    )]
    SessionUnavailableForResume {
        session_id: meerkat_core::SessionId,
        reason: SessionResumeUnavailableReason,
        /// Blocking runtime lifecycle, when one was recorded.
        runtime_state: Option<String>,
        /// Full owner-issued verdict when this error came from the typed
        /// operational resume pipeline.
        verdict: Option<Box<crate::runtime::SessionResumeRejection>>,
    },

    /// A mob member with the given ID already exists.
    #[error("mob member already exists: {0}")]
    MemberAlreadyExists(AgentIdentity),

    /// A spawn that had not reached custody (still preparing, or staged and
    /// provisioning) was canceled by a lifecycle transition (stop, complete,
    /// reset, destroy, shutdown, or retire of the member). Nothing of it was
    /// seated; `reason` names the transition.
    #[error("spawn canceled for '{member_id}': {reason}")]
    SpawnCanceled {
        member_id: AgentIdentity,
        reason: String,
    },

    /// The mob's admission is closed (D67), for any
    /// [`MobAdmissionCloseReason`](crate::runtime::MobAdmissionCloseReason):
    /// the refused command was not admitted. Admitted work is unaffected, and
    /// the close itself refuses no Stop, Shutdown, Destroy, retirement,
    /// cancellation or read.
    #[error("mob '{mob_id}' admission is closed ({reason})")]
    AdmissionClosed {
        mob_id: MobId,
        reason: crate::runtime::MobAdmissionCloseReason,
    },

    /// Desired member material is committed but its replacement executor is
    /// not yet active. New work remains closed until identity convergence.
    #[error("member '{member_id}' admission is closed for identity convergence")]
    IdentityConvergenceAdmissionClosed { member_id: AgentIdentity },

    /// The durable child session committed, but ordinary resume provisioning
    /// did not seat the requested member identity. The child session id is
    /// retained so an operator can retry resume rather than losing the fork.
    #[error(
        "durable fork session '{fork_session_id}' committed for member '{member_id}', but resume provisioning failed: {reason}"
    )]
    ForkMemberProvisionFailed {
        member_id: AgentIdentity,
        fork_session_id: meerkat_core::SessionId,
        reason: String,
    },

    /// Receiver-side compact helper projection rejected an invalid label or
    /// byte bound before reading/storing any worker payload.
    #[error("invalid bounded helper result request: {reason}")]
    InvalidBoundedHelperResult { reason: String },

    /// Canonical member/session authority changed while the receiver was
    /// projecting an exact bounded helper result. The worker remains intact;
    /// callers may retry the read against a stable terminal observation.
    #[error("bounded helper result for member '{member_id}' is temporarily unavailable: {reason}")]
    BoundedHelperResultUnavailable {
        member_id: AgentIdentity,
        reason: String,
    },

    /// Exact bounded projection succeeded, but retirement did not. The member
    /// and durable session identities remain explicit recovery authority.
    #[error(
        "bounded helper result was certified for member '{member_id}' and session '{session_id}', but retirement failed: {reason}"
    )]
    BoundedHelperRetirementFailed {
        member_id: AgentIdentity,
        session_id: meerkat_core::SessionId,
        reason: String,
    },

    /// The mob member's profile does not allow external turns.
    #[error("mob member is not externally addressable: {0}")]
    NotExternallyAddressable(AgentIdentity),

    /// The requested lifecycle state transition is invalid.
    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition { from: MobState, to: MobState },

    /// [`MobHandle::drain_stopped_runs`](crate::MobHandle::drain_stopped_runs)
    /// runs only on a Stopped mob, whose members' run starts are held.
    #[error("draining runs requires a stopped mob, but the mob is {state}")]
    DrainRequiresStoppedMob { state: MobState },

    /// A wiring operation failed.
    #[error("wiring error: {0}")]
    WiringError(String),

    /// The definition text could not be parsed as a mob definition.
    #[error("mob definition parse error: {0}")]
    DefinitionParse(#[from] toml::de::Error),

    /// A `[profiles.<name>]` table declares a key that names a platform
    /// concept a mob profile cannot honour (`system_prompt` and its aliases).
    /// Refused at definition load: `Profile` cannot use `deny_unknown_fields`,
    /// so without this check the key would be dropped silently and the member
    /// would run on the default prompt.
    #[error(
        "profile '{profile}' declares '{key}', which a mob profile cannot honour: {}",
        crate::profile::UnsupportedProfileKey::HINT
    )]
    UnsupportedProfileKey {
        profile: ProfileName,
        key: crate::profile::UnsupportedProfileKey,
    },

    /// An external provisioning attempt could not certify its compensation
    /// complete. The exact PeerId/operation reservation remains quarantined
    /// whenever possible; callers must keep cleanup authority open and fail
    /// stop rather than falsely certifying rollback.
    #[error("external member cleanup is uncertain: {reason}")]
    ExternalMemberCleanupUncertain { reason: String },

    /// Retirement could not converge machine topology and reciprocal trust.
    /// The durable retiring roster anchor must be retained so the exact
    /// cleanup can be retried instead of publishing a false terminal event.
    #[error("retirement topology cleanup incomplete: {0}")]
    RetirementTopologyIncomplete(String),

    /// Retirement was durably admitted and remains non-routable, but the
    /// exact runtime teardown could not converge before the owner deadline.
    /// The roster, session, and runtime witnesses are retained for retry.
    #[error(
        "member retirement remains in progress for session '{session_id}' at {stage}; the owner deadline was reached"
    )]
    RetirementInProgress {
        session_id: meerkat_core::SessionId,
        stage: String,
    },

    /// Peer-only retirement has no controller-local session identity. The
    /// durable member identity remains the exact retry anchor while the same
    /// bounded lifecycle cleanup continues.
    #[error(
        "member retirement remains in progress for peer-only member '{member_id}' at {stage}; the owner deadline was reached"
    )]
    MemberRetirementInProgress {
        member_id: AgentIdentity,
        stage: String,
    },

    /// The exact retirement command is process-owned and single-flight, but
    /// the actor has not yet durably published Retiring. This is retryable
    /// busy state, not authority to claim that teardown was already admitted.
    #[error(
        "member retirement admission is still pending for '{member_id}' at {stage}; no durable Retiring claim is implied"
    )]
    MemberRetirementAdmissionPending {
        member_id: AgentIdentity,
        stage: String,
    },

    /// The member's retirement durably started and then stopped at `stage`
    /// with the typed `cause`. The member stays `Retiring`, owned by the mob
    /// actor's stuck-retirement registry. Retrying a plain retirement does not
    /// drive it; `MobHandle::redrive_retirement` or a mob resume does. Its
    /// structured data carries the cause's text as `cause` and, when the
    /// cause has structured data, that data as `cause_data`.
    #[error("member retirement for '{member_id}' is stuck at {stage}: {cause}")]
    MemberRetirementStuck {
        member_id: AgentIdentity,
        stage: String,
        cause: Arc<MobError>,
    },

    /// A mob Shutdown interrupted the member's in-flight retirement at
    /// `stage`. A durably started retirement stays `Retiring` and is driven
    /// again on the next resume.
    #[error("member retirement for '{member_id}' was interrupted by shutdown at {stage}")]
    RetirementInterrupted {
        member_id: AgentIdentity,
        stage: String,
    },

    /// One process-owned exact-incarnation retirement is shared by concurrent
    /// callers. The actor produces a single owned error; this transparent Arc
    /// preserves that exact typed cause for every joined observer without
    /// re-running the retirement command or reducing it to display text.
    #[error(transparent)]
    SharedRetirementFailure(Arc<MobError>),

    /// One process-owned explicit lifecycle operation is shared by concurrent
    /// handle clones. The actor produces one owned error; this transparent
    /// Arc preserves the exact typed cause for every joined observer without
    /// reducing it to display text or replaying the lifecycle command.
    #[error(transparent)]
    SharedLifecycleFailure(Arc<MobError>),

    /// Supervisor rotation reached one or more remote members but did not
    /// complete, so local supervisor authority stayed at the pre-rotation
    /// epoch.
    #[error(
        "supervisor rotation incomplete: failed after {rotated_peer_count} remote peer(s) accepted attempted epoch {attempted_epoch}; local authority remains at epoch {previous_epoch}; rollback_succeeded={rollback_succeeded}; pending_authority_recorded={pending_authority_recorded}; failure: {reason}"
    )]
    SupervisorRotationIncomplete {
        previous_epoch: u64,
        attempted_epoch: u64,
        attempted_public_peer_id: String,
        rotated_peer_count: usize,
        rollback_succeeded: bool,
        pending_authority_recorded: bool,
        rollback_error: Option<String>,
        reason: String,
    },

    /// The current supervisor authority has consumed the final representable
    /// epoch. Rotation must refuse before minting a candidate, mutating durable
    /// metadata, or contacting any member/host; wrapping to zero would reopen
    /// stale-authority acceptance.
    #[error(
        "supervisor epoch space is exhausted at {current_epoch}; refusing to wrap or reuse authority"
    )]
    SupervisorEpochExhausted { current_epoch: u64 },

    /// A supervisor bridge command was rejected by the remote member.
    #[error("bridge command rejected ({cause:?}): {reason}")]
    BridgeCommandRejected {
        cause: BridgeRejectionCause,
        reason: String,
    },

    /// Plane-(b) control-scope denial (§8). Carries the typed denial so no
    /// surface reconstructs `{required, presented}` from a display string —
    /// the Display rendering is human-facing only; the typed fields are the
    /// contract.
    #[error("control scope denied: requires {}", .0.required_name())]
    ScopeDenied(ScopeDenial),

    /// A supervisor bridge request reached its reply deadline (typed so the
    /// materialize dispatcher's bounded-resend classification never string-
    /// matches an `Internal` rendering — ADJ-4).
    #[error("supervisor bridge request '{request_envelope_id}' timed out after {timeout_ms}ms")]
    BridgeRequestTimedOut {
        request_envelope_id: String,
        timeout_ms: u64,
    },

    /// The member failed to restore durable session state and is broken until repaired.
    ///
    /// `hold` is meerkat's typed durable resume hold when the restore failure
    /// WAS a hold (a WholeBlob document that needs the sanctioned
    /// audited-endpoint repair, a held tail, a refused recovery, quarantined
    /// evidence): the session is intact and withheld, and the hold class says
    /// what clears it. Hosts classify on it (or on
    /// [`MobError::durable_resume_hold`]) instead of parsing `reason`.
    #[error(
        "member {member_id} failed to restore {}: {reason}",
        format_member_restore_target(.session_id.as_ref())
    )]
    MemberRestoreFailed {
        member_id: AgentIdentity,
        session_id: Option<meerkat_core::types::SessionId>,
        reason: String,
        hold: Option<meerkat_core::service::DurableResumeHold>,
    },

    /// The mob's startup recovery could not restore member `member_id`'s
    /// session `session_id`, so the mob actor stopped before it admitted any
    /// command. `cause` is the typed failure, for example
    /// `SessionError::ServedElsewhere` while another runtime owner on the
    /// realm hosts the session. Recovery wrote nothing for that session;
    /// start the mob again once the cause clears.
    #[error("mob startup could not restore member {member_id} session {session_id}: {cause}")]
    MemberStartupRecoveryFailed {
        member_id: AgentIdentity,
        session_id: meerkat_core::types::SessionId,
        cause: Arc<MobError>,
    },

    /// The mob's actor fail-stopped because the outcome of a durable mutation
    /// stayed uncertain: `cause` is that mutation's own error (the first one,
    /// when several stayed uncertain). The actor closed its admission with
    /// [`MobAdmissionCloseReason::DurabilityFailStop`](crate::runtime::MobAdmissionCloseReason::DurabilityFailStop)
    /// and admits no command until a restart replays the mob's durable state.
    ///
    /// It classifies as that close, not as its cause: the actionable fact is
    /// that the mob needs a restart. It carries no durable resume hold, even
    /// when its cause does, so a host does not park a member on a hold that
    /// only the restart releases.
    #[error("mob '{mob_id}' fail-stopped after an uncertain durable mutation: {cause}")]
    DurabilityFailStop { mob_id: MobId, cause: Arc<MobError> },

    /// Member `member_id`'s session `session_id` has a durable tail past its
    /// committed runtime boundary that the store-owned recovery could not
    /// prove: the recovery kept the tail intact, so the session still reads
    /// as a revision conflict at that boundary. Nothing was written; the tail
    /// stays for reconciliation. The structured data carries the
    /// `tail_held_for_recovery` resume hold.
    #[error(
        "member {member_id} session {session_id} has a durable tail its recovery could not prove; the tail is kept for reconciliation"
    )]
    MemberDurableTailUnprovable {
        member_id: AgentIdentity,
        session_id: meerkat_core::types::SessionId,
    },

    /// The member's runtime registration is durability-degraded: a durable
    /// commit could not be reconciled with the live shell, so the runtime
    /// refuses every ordinary input until a registration-authorized cold
    /// reload replaces the registration. Delivery is rejected before dispatch
    /// so a degraded member never costs the caller an admission budget.
    /// `MobHandle::reload_member_registration` is the non-destructive repair.
    #[error("member {member_id} requires a runtime reload before it can accept work: {reason}")]
    MemberReloadRequired {
        member_id: AgentIdentity,
        reason: String,
    },

    /// The member's session has no committed runtime attachment, so its
    /// runtime registration cannot serve work: it is absent or detached (for
    /// example a runtime-loop teardown whose unregister never completed).
    /// Dispatch refuses typed from the same attachment fact the registration
    /// reload reads; `MobHandle::reload_member_registration` completes any
    /// pending unregister and re-attaches the same session (#1248).
    #[error(
        "session '{session_id}' has no committed runtime attachment; explicit create or resume is required ({detachment})"
    )]
    MemberRuntimeDetached {
        session_id: meerkat_core::types::SessionId,
        detachment: MemberRuntimeDetachment,
    },

    /// The member's per-member admission lane already holds the maximum
    /// number of parked deliveries. One admission is in flight and `depth`
    /// deliveries wait behind it; the caller must retry later. This is
    /// backpressure for exactly one member, never a verdict on the mob.
    #[error("member {member_id} admission backlog is full ({depth} parked deliveries)")]
    MemberAdmissionBacklogFull {
        member_id: AgentIdentity,
        depth: usize,
    },

    /// `reload_member_registration` refused to discard the member's live
    /// runtime shell because the durable session authority it would reload
    /// from is not currently readable (store still failing, contradictory,
    /// archived, or the session service holds no durable sessions). The
    /// member stays degraded and repairable instead of being marked Broken
    /// by a revival that could only fail.
    /// A held spawn ([`crate::MobHandle::spawn_held_until`]) was refused: only a
    /// fresh, locally provisioned member through a provisioner that
    /// supports holding can be held.
    #[error("held spawn unsupported: {reason}")]
    HeldSpawnUnsupported { reason: String },

    /// A held spawn was aborted before activation: by the caller, because its
    /// handle was dropped, or because its caller went away while the setup
    /// was still being constructed. The member was not published.
    /// `cleanup` is `None` when the abort confirmed its cleanup (or nothing
    /// existed to clean up yet), or the abort's own error (cleanup uncertain
    /// or failed).
    #[error("held spawn of '{member}' was aborted before activation{}", held_cleanup_suffix(cleanup.as_deref()))]
    HeldSpawnAborted {
        member: String,
        cleanup: Option<Box<MobError>>,
    },

    /// A held spawn reached the caller's setup deadline without an accepted
    /// activation, before or at its hold point; its setup was stopped and
    /// aborted, and the member was not published. A spawn whose command was
    /// still waiting for admission was never delivered, so nothing existed
    /// to clean up. `cleanup` as for [`MobError::HeldSpawnAborted`].
    #[error("held spawn of '{member}' expired at its setup deadline without activation{}", held_cleanup_suffix(cleanup.as_deref()))]
    HeldSpawnExpired {
        member: String,
        cleanup: Option<Box<MobError>>,
    },

    #[error("member session {session_id} reload refused: {reason}")]
    MemberReloadRefused {
        session_id: meerkat_core::types::SessionId,
        reason: String,
    },

    /// A member was asked to be held before its publication where holding
    /// is not supported: by this provisioner, or for this provision (only a
    /// fresh runtime-backed member whose host selects its registration is
    /// held). Nothing was provisioned.
    #[error("held member provision unsupported: {reason}")]
    HeldProvisionUnsupported { reason: String },

    /// `reload_member_registration` did not finish `stage` within
    /// `MEMBER_RELOAD_TOTAL_TIMEOUT`. Every completed step is durable and
    /// idempotent; a repeat call observes the current state
    /// (`NotDegraded` once the reload landed).
    #[error("member session {session_id} reload timed out at {stage}")]
    MemberReloadTimedOut {
        session_id: meerkat_core::types::SessionId,
        stage: &'static str,
    },

    /// A bounded actor command did not complete `stage` before its caller
    /// deadline. Closing the reply channel skips a delivery still queued on
    /// the actor or parked in a member lane, but does not cancel work already
    /// started. This observation alone proves neither execution fate nor
    /// retry safety; those require runtime-owned admission evidence.
    #[error("actor command {command_kind} exceeded its deadline at {stage}")]
    ActorCommandTimedOut {
        command_kind: &'static str,
        stage: &'static str,
    },

    /// A generated MobMachine -> runtime route reached its consumer, which
    /// rejected the typed input. The MobMachine has already absorbed the
    /// generated refusal closure before this error is returned; `kind` and
    /// `refusal_code` are typed/stable and `reason` is display detail only.
    #[error("{kind} consumer refused bridge session {session_id} [{refusal_code}]: {reason}")]
    RuntimeEffectRefused {
        kind: RuntimeEffectKind,
        session_id: meerkat_core::types::SessionId,
        refusal_code: String,
        reason: String,
    },

    /// Waiting for kickoff completion timed out.
    #[error("kickoff wait timed out")]
    KickoffWaitTimedOut {
        pending_member_ids: Vec<AgentIdentity>,
    },

    /// Waiting for startup readiness timed out.
    #[error("member ready wait timed out")]
    ReadyWaitTimedOut {
        pending_member_ids: Vec<AgentIdentity>,
    },

    /// The mob definition failed validation.
    #[error("definition error: {}", format_diagnostics(.0))]
    DefinitionError(Vec<Diagnostic>),

    /// Referenced flow does not exist.
    #[error("flow not found: {0}")]
    FlowNotFound(FlowId),

    /// Run failed with a reason.
    #[error("flow failed for run {run_id}: {reason}")]
    FlowFailed { run_id: RunId, reason: String },

    /// Referenced run does not exist.
    #[error("run not found: {0}")]
    RunNotFound(RunId),

    /// Run was canceled.
    #[error("run canceled: {0}")]
    RunCanceled(RunId),

    /// Flow turn timed out while awaiting terminal transport outcome.
    #[error("flow turn timed out")]
    FlowTurnTimedOut,

    /// A frame-aware flow exceeded its configured nesting depth.
    #[error(
        "loop '{loop_id}' would exceed max_frame_depth={max_frame_depth} (current depth={current_depth})"
    )]
    FrameDepthLimitExceeded {
        loop_id: LoopId,
        max_frame_depth: u32,
        current_depth: u32,
    },

    /// The selected mob run store cannot provide frame-aware atomic persistence.
    #[error("mob run store cannot atomically persist frame operation '{operation}'")]
    FrameAtomicPersistenceUnavailable { operation: FrameAtomicOperation },

    /// Spec revision compare-and-swap failed.
    #[error("spec revision conflict for mob {mob_id}: expected {expected:?}, actual {actual}")]
    SpecRevisionConflict {
        mob_id: MobId,
        expected: Option<u64>,
        actual: u64,
    },

    /// Durable definition authority and its spec projection disagree at boot.
    #[error(
        "mob {mob_id} definition projection mismatch ({kind}): authority epoch {authority_epoch}, projection revision {projection_revision}"
    )]
    MobDefinitionProjectionMismatch {
        mob_id: MobId,
        authority_epoch: u64,
        projection_revision: u64,
        kind: MobDefinitionProjectionMismatchKind,
    },

    /// Canonical definition authority changed after a storage-minted preflight snapshot.
    #[error(
        "mob definition authority changed before resume: expected epoch {expected_epoch} at event cursor {expected_event_cursor}, actual epoch {actual_epoch:?} at event cursor {actual_event_cursor:?}"
    )]
    MobDefinitionAuthorityChanged {
        expected_epoch: u64,
        expected_event_cursor: u64,
        actual_epoch: Option<u64>,
        actual_event_cursor: Option<u64>,
    },

    /// Schema validation failed for a step output.
    #[error("schema validation failed for step {step_id}: {message}")]
    SchemaValidation { step_id: StepId, message: String },

    /// Not enough targets to satisfy dispatch/collection policy.
    #[error("insufficient targets for step {step_id}: required {required}, available {available}")]
    InsufficientTargets {
        step_id: StepId,
        required: u8,
        available: usize,
    },

    /// Role-targets admission refused a flow run. Nothing was written,
    /// provisioned or dispatched.
    #[error("flow '{flow_id}' role targets refused: {refusal}")]
    FlowRoleTargetsRefused {
        flow_id: FlowId,
        refusal: FlowRoleTargetRefusal,
    },

    /// A step of a role-targeted run can no longer meet its collection policy
    /// with the bound members of its role that are still Active and still
    /// hold the role. The run fails before the step dispatches any turn: a
    /// bound run never substitutes another member and never waits for one.
    #[error(
        "bound targets unavailable for step {step_id} of run {run_id}: required {required}, available [{}], unavailable [{}]",
        format_identities(.available),
        format_identities(.unavailable)
    )]
    BoundFlowTargetsUnavailable {
        run_id: RunId,
        step_id: StepId,
        required: usize,
        available: Vec<AgentIdentity>,
        unavailable: Vec<AgentIdentity>,
    },

    /// A keyed flow run admission named a key whose run was admitted with
    /// other activation params or other role targets. The first run stands;
    /// no second run was admitted.
    #[error(
        "flow run '{run_id}' of flow '{flow_id}' was admitted under this idempotency key with other activation params or role targets"
    )]
    FlowRunIdempotencyConflict { run_id: RunId, flow_id: FlowId },

    /// Topology policy denied a dispatch edge.
    #[error("topology violation: {from_role} -> {to_role}")]
    TopologyViolation {
        from_role: ProfileName,
        to_role: ProfileName,
    },

    /// A bridge accepted the delivery command but rejected the member input.
    #[error("bridge delivery rejected ({cause}): {reason}")]
    BridgeDeliveryRejected {
        cause: Box<meerkat_contracts::wire::supervisor_bridge::BridgeDeliveryRejectionCause>,
        reason: String,
    },

    /// Supervisor escalation happened.
    #[error("supervisor escalation: {0}")]
    SupervisorEscalation(String),

    /// A spawn carried `SpawnMemberSpec::resume_provider_params` but does not
    /// resume a durable session (a fresh, policy or remote spawn).
    #[error("'{identity}' carries resume provider params but does not resume a durable session")]
    ResumeProviderParamsRequireResume { identity: AgentIdentity },

    /// Operation is not supported for the member's runtime mode.
    #[error("unsupported for runtime mode {mode}: {reason}")]
    UnsupportedForMode {
        mode: crate::MobRuntimeMode,
        reason: String,
    },

    /// The named source member cannot serve a forked-participant capability
    /// operation. Controller-side routing refusal only — the source-owner
    /// service and the owning host still decide capability lifecycle legality.
    #[error("forked participant source '{member_id}' is not eligible: {rejection}")]
    ForkedParticipantSourceIneligible {
        member_id: AgentIdentity,
        rejection: ForkedParticipantSourceRejection,
    },

    /// The source-owner capability service refused the operation. The typed
    /// service error is retained as the source so callers keep the machine's
    /// own verdict instead of a display string.
    #[error("forked participant capability operation refused")]
    ForkedParticipantRefused(#[source] Box<crate::forked_participant::ForkedParticipantError>),

    /// An ordinary resume named a capability-owned fork session without an
    /// attachment receipt.
    #[error(
        "session {session_id} is owned by a forked-participant capability; use the capability-aware attached spawn"
    )]
    ForkedParticipantResumeRequiresAttachment {
        /// Protected fork session identity.
        session_id: meerkat_core::SessionId,
    },

    /// Explicit leases for a host-owned capability must be part of the V6
    /// host materialization/release protocol, which this local-only surface
    /// deliberately does not proxy.
    #[error(
        "forked participant {operation} on a host route is unsupported: remote attachment must be coupled to V6 Materialize/Release"
    )]
    ForkedParticipantRemoteLeaseUnsupported {
        /// Lease verb that was refused.
        operation: ForkedParticipantLeaseOperation,
    },

    /// A host-owned capability cannot be seated because its owning host is not
    /// usable in this mob. Decided from machine host facts BEFORE any bridge
    /// traffic, so an unbound or pre-V6 host is never contacted.
    #[error("forked participant capability owner host '{host_id}' cannot seat it: {rejection}")]
    ForkedParticipantOwnerHostUnavailable {
        /// Capability's own owner host identity.
        host_id: String,
        /// Typed reason, resolved from recorded MobMachine host facts.
        rejection: ForkedParticipantOwnerHostRejection,
    },

    /// The target spawn spec presented to the capability-aware attached spawn
    /// is not admissible. The branch INHERITS the source's effective execution
    /// context, so a spec that would widen or re-point it is refused before
    /// the capability's lifecycle machine is touched.
    #[error("forked participant attached spawn spec rejected: {detail}")]
    ForkedParticipantAttachedSpawnSpecRejected {
        /// What was wrong with the presented target spec.
        detail: String,
    },

    /// The capability attachment was admitted, but the mob could not durably
    /// record the association or its reconciliation obligation.
    ///
    /// The attachment is deliberately NOT blind-released here: without a
    /// durable row, a release that itself fails would leave nothing behind to
    /// converge from. The typed detail names the exact attachment so an
    /// operator (or the owner's own pending-attached enumeration) can drive
    /// reconciliation.
    #[error(
        "forked participant attachment '{attachment_id}' could not be durably recorded: {detail}"
    )]
    ForkedParticipantAttachmentCustodyUnrecorded {
        /// Attachment identity that remains outstanding.
        attachment_id: String,
        /// Store-reported failure detail.
        detail: String,
    },

    /// A capability attachment seated in this mob outlived the teardown that
    /// should have released it: session/runtime teardown completed but the
    /// capability release did not, so cleanup must not report completion.
    #[error(
        "forked participant attachment '{attachment_id}' release after teardown of '{member_id}' is unproven: {detail}"
    )]
    ForkedParticipantAttachmentReleaseUnproven {
        /// Member whose teardown owed the release.
        member_id: AgentIdentity,
        /// Attachment identity that remains outstanding.
        attachment_id: String,
        /// Typed failure detail.
        detail: String,
    },

    /// A peer-only lifecycle effect requires the explicit durable supervisor
    /// protocol rotation ceremony before V5 direct-member authority exists.
    #[error(
        "operation '{operation}' requires supervisor protocol {required:?}; current durable protocol is {current:?}"
    )]
    SupervisorProtocolUpgradeRequired {
        operation: String,
        current: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion,
        required: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion,
    },

    /// Supervisor rotation already committed the V5 authority, but one or
    /// more direct peer-member bearers have not yet converged. Retrying the
    /// same rotation request drains adoption at the committed epoch; it must
    /// not mint another supervisor authority.
    #[error(
        "supervisor authority is committed at epoch {current_epoch}, but direct-member adoption remains pending: {reason}"
    )]
    DirectMemberAdoptionPending { current_epoch: u64, reason: String },

    /// A member is missing a required runtime capability for the requested operation.
    #[error("mob member {member_id} missing required capability {capability}: {context}")]
    MissingMemberCapability {
        member_id: AgentIdentity,
        capability: MobMemberCapability,
        context: &'static str,
    },

    /// Injected context cannot be delivered on this work-dispatch path.
    ///
    /// The autonomous inbox path flows through comms plain events (no
    /// user-channel transcript boundary), and steer dispatch realizes as
    /// live system-context appends; neither can materialize typed
    /// injected-context messages before the work content. Fail closed
    /// rather than silently dropping host-provided context.
    #[error("injected context undeliverable to member {member_id}: {reason}")]
    InjectedContextUndeliverable {
        member_id: AgentIdentity,
        reason: &'static str,
    },

    /// A caller-supplied interaction id already has durable placed-completion
    /// history. The member host retains an ACK/cancel tombstone for that key,
    /// so reopening it as fresh work could deduplicate without producing a
    /// new terminal sidecar.
    #[error("placed interaction id has already been used: {interaction_id}")]
    PlacedInteractionIdAlreadyUsed { interaction_id: String },

    #[error("placed interaction id must be a canonical non-nil UUID: {interaction_id}")]
    InvalidPlacedInteractionId { interaction_id: String },

    #[error("placed completion was durably closed after certified delivery rejection")]
    PlacedCompletionDeliveryRejected,

    #[error("placed completion was durably closed after the member host certified no effect")]
    PlacedCompletionHostNoEffect,

    #[error("placed completion was durably closed after the member host cancelled the input")]
    PlacedCompletionHostCancelled,

    #[error("placed completion custody was durably disposed after member release")]
    PlacedCompletionDisposed,

    /// Retryable lifecycle barrier: exact host cleanup continues outside the
    /// volatile lifecycle RPC while durable origin authority remains closed.
    #[error("placed completion cleanup is still pending (pending={pending}, resolved={resolved})")]
    PlacedCompletionCleanupPending { pending: usize, resolved: usize },

    /// Retryable lifecycle barrier: durable kickoff cancellation has closed
    /// origin and the actor-lifetime reconciler is still obtaining exact host
    /// no-effect, cancellation, terminal, or ACK authority.
    #[error("placed kickoff cleanup is still pending (pending={pending}, resolved={resolved})")]
    PlacedKickoffCleanupPending { pending: usize, resolved: usize },

    /// Retryable lifecycle barrier: all kickoff origin is durably closed and
    /// exact timeout-bounded member interrupts are running off the actor loop.
    #[error("autonomous stop interrupts are still pending (pending={pending})")]
    AutonomousStopInterruptsPending { pending: usize },

    /// A durable lifecycle operation has fenced fresh work/runtime origin.
    #[error("mob lifecycle operation is still in progress: {intent}")]
    LifecycleOperationPending { intent: String },

    /// A process-owned lifecycle operation retained authority after durable
    /// admission, but its host-owned work made no observable member progress
    /// for one patience window. The exact operation remains joinable on retry.
    #[error("mob lifecycle operation made no progress at {stage}: {intent} (member={member_id:?})")]
    LifecycleOperationProgressStalled {
        intent: String,
        member_id: Option<AgentIdentity>,
        stage: &'static str,
    },

    /// A lifecycle command could not enter the actor mailbox before its
    /// absolute deadline. No lifecycle authority was admitted or retained.
    #[error("mob lifecycle operation admission is still pending at {stage}: {intent}")]
    LifecycleOperationAdmissionPending { intent: String, stage: &'static str },

    /// Operation blocked by reset barrier.
    #[error("reset barrier active")]
    ResetBarrier,

    /// The generated MobMachine rejected an operation.
    #[error("mob machine rejected {context}: {reason}")]
    MobMachineRejected {
        context: &'static str,
        reason: String,
    },

    /// A storage operation failed.
    #[error("storage error: {0}")]
    StorageError(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A session service operation failed.
    #[error("session error: {0}")]
    SessionError(#[from] meerkat_core::service::SessionError),

    /// Member provisioning reached a typed process-incarnation-terminal
    /// boundary. The cause owner, not the actor's display formatter, decides
    /// whether the failure is permanent for this incarnation.
    #[error("member provision failed: {cause}")]
    MemberProvisionFailed { cause: MemberProvisionFailureCause },

    /// A comms operation failed.
    #[error("comms error: {0}")]
    CommsError(#[from] meerkat_core::comms::SendError),

    /// A runtime-backed member turn reached an external callback boundary.
    #[error("callback pending for session {session_id} on tool '{tool_name}'")]
    CallbackPending {
        session_id: meerkat_core::types::SessionId,
        tool_use_id: String,
        tool_name: String,
        args: serde_json::Value,
        /// The call's dedupe key, as on
        /// [`meerkat_core::error::PendingCallbackToolCall`]: a host answering
        /// the call sees the same key if a resumed run asks again. Boxed so
        /// the error stays small.
        dedupe_key: Option<Box<meerkat_core::ToolCallDedupeKey>>,
    },
    /// A member turn is waiting on one exact multi-callback result set.
    #[error(
        "callback batch pending for session {session_id} ({pending_tool_calls_len} tool calls)",
        pending_tool_calls_len = .pending_tool_calls.len()
    )]
    CallbackBatchPending {
        session_id: meerkat_core::types::SessionId,
        pending_tool_calls: Vec<meerkat_core::error::PendingCallbackToolCall>,
    },

    /// The fence token does not match the member's current incarnation.
    #[error("stale fence token for {runtime_id}: expected {expected}, got {actual}")]
    StaleFenceToken {
        runtime_id: AgentRuntimeId,
        expected: FenceToken,
        actual: FenceToken,
    },

    /// A scope-bound submit named a member session that is no longer the
    /// member's current session binding. Generated SubmitWork authority
    /// refused it before any admission effect; nothing was delivered, and
    /// the submit was never retargeted to the current session.
    #[error(
        "stale delivery scope for {agent_identity}: expected session {expected_session}, current {}",
        actual_session.as_ref().map_or_else(|| "none".to_string(), ToString::to_string)
    )]
    StaleDeliveryScope {
        agent_identity: AgentIdentity,
        expected_session: meerkat_core::types::SessionId,
        actual_session: Option<meerkat_core::types::SessionId>,
    },

    /// A delivery scope cannot be captured for this member (for example a
    /// peer-only member with no session binding to scope a delivery to).
    #[error("delivery scope unavailable for {agent_identity}: {reason}")]
    DeliveryScopeUnavailable {
        agent_identity: AgentIdentity,
        reason: String,
    },

    /// A durable remote member-operator request reached actor execution after
    /// its exact placed residency was superseded.
    #[error("stale member-operator execution authority for {member_id}: {reason}")]
    StaleMemberOperatorAuthority {
        member_id: AgentIdentity,
        reason: String,
    },

    /// A caller supplied an event replay cursor beyond the store frontier.
    #[error("stale mob event cursor: requested {after_cursor}, latest {latest_cursor}")]
    StaleEventCursor {
        after_cursor: u64,
        latest_cursor: u64,
    },

    /// The referenced work unit does not exist.
    #[error("work not found: {0}")]
    WorkNotFound(WorkRef),

    /// Per-unit work cancellation is not realized: no work-tracking ledger
    /// backs `cancel_work`, so there is no authority that can locate and
    /// cancel an individual submitted unit. Returned instead of a phantom
    /// `WorkNotFound` so the advertised `mob/cancel_work` surface fails
    /// closed with an honest "unsupported" signal rather than claiming a
    /// search-and-miss that never happened. Callers that need to cancel
    /// in-flight work should use member-scoped `cancel_all_work`.
    #[error("per-unit work cancellation is unsupported for {0}: no work-tracking ledger is wired")]
    WorkCancellationUnsupported(WorkRef),

    /// The mob actor command channel closed before accepting a command: the
    /// actor has exited or is exiting. While a Shutdown drains its own work,
    /// the actor also answers its internal requests this way, so the tasks
    /// it joins can finish; other requests then get
    /// [`Self::ShutdownInProgress`] (or [`Self::AdmissionClosed`] for a
    /// command the close refuses).
    #[error("mob actor command channel closed")]
    ActorCommandChannelClosed,

    /// The mob actor accepted a command but dropped the reply channel.
    #[error("mob actor reply channel closed")]
    ActorReplyChannelClosed,

    /// A bridge session could not be located in any live mob authority roster
    /// (and is not owned by service-reported or persisted authority either),
    /// so live-handle retirement cannot resolve a member to retire.
    ///
    /// This is a typed recovery-class observation: callers that already hold
    /// independent evidence the bridge session is mob-owned (e.g. bridge-session
    /// scoped mobs) may proceed to scoped cleanup instead of failing.
    #[error("bridge session not found in any live mob authority: {bridge_session_id}")]
    BridgeSessionNotInLiveAuthority { bridge_session_id: String },

    /// A mob-member comms (peer) routing name could not be rendered because a
    /// component failed the [`meerkat_core::MemberCommsName`] slug rule.
    ///
    /// The routing-name shape has exactly one fail-closed owner; rendering
    /// surfaces the typed component fault here instead of reconstructing a raw
    /// `mob_id/role/member` join that the owner already rejected.
    #[error("invalid mob member comms name: {0}")]
    MemberCommsName(#[from] meerkat_core::MemberCommsNameError),

    /// A flow/loop condition could not be evaluated to a definite boolean
    /// because it referenced an absent/invalid context path or compared
    /// non-comparable operands.
    ///
    /// Surfaced as a typed fault (`location` names the owning step or loop)
    /// rather than silently evaluating the condition to `false` and skipping
    /// the step / mis-deciding the loop-until.
    #[error("condition evaluation failed for {location}: {reason}")]
    ConditionEval { location: String, reason: String },

    /// The MobMachine spawn ladder resolved a non-`Allowed` admission for a
    /// spawn (multi-host placement/portability/capability denial arms).
    ///
    /// The typed admission kind is the machine's verdict verbatim
    /// (`SpawnMemberAdmissionResolved { admission }`); the shell never
    /// re-derives or re-words it (DEC-P3F-6).
    #[error("spawn admission denied: {admission:?}")]
    SpawnMemberAdmissionDenied {
        admission: crate::machines::mob_machine::MobSpawnMemberAdmissionKind,
    },

    /// An authenticated host attempted to replace a capability contract that
    /// existing placed residency/custody still depends on. Kept distinct from
    /// transport unavailability: the host answered, but is incompatible.
    #[error("host '{host_id}' cannot preserve its active capability contract; missing {missing:?}")]
    HostCapabilityContractViolation {
        host_id: String,
        missing: Vec<String>,
    },

    /// A fork admitted in its source member's own turn (`CallerTurn`) owes
    /// its job to that member: the job's owner session must be the source
    /// member's bridge session. Refused before anything is forked or seated.
    #[error(
        "a fork in the turn of '{source_member_id}' binds its job to that member's session {source_session_id}, not {owner_session_id}"
    )]
    ForkJobOwnerNotSource {
        source_member_id: AgentIdentity,
        source_session_id: meerkat_core::SessionId,
        owner_session_id: meerkat_core::SessionId,
    },

    /// A fork source member's session history is unavailable (W-G retype:
    /// typed from birth, never an `Internal` masquerade).
    #[error("fork source '{source_member_id}' is unavailable: {cause}")]
    ForkSourceUnavailable {
        source_member_id: String,
        cause: ForkSourceUnavailableCause,
    },

    /// A flow step was rejected at dispatch classification (§18.8:1000/:1001
    /// via `ClassifyFlowStepDispatch`, phase 6): the machine refused the
    /// dispatch BEFORE any delivery. One machine arm, one variant — the
    /// local and remote overlay+autonomous cases produce THE SAME cause
    /// (§18.11:1043 "same cause local and remote").
    #[error("flow step dispatch rejected for '{target}': {kind}")]
    FlowStepDispatchRejected {
        target: AgentIdentity,
        kind: FlowStepDispatchRejectKind,
    },

    /// An explicit runtime adapter is a different live runtime owner than
    /// the session service's, even if both use the same runtime store. One
    /// owner of record holds a mob's sessions, so the build is refused before
    /// anything is provisioned (#1550).
    #[error(
        "explicit mob runtime adapter is a different runtime owner than the session service's runtime owner"
    )]
    RuntimeOwnerConflict,

    /// A member's profile names a tool bundle the mob's builder does not
    /// register, for example a host bundle the host has withdrawn since the
    /// mob was created. The member is not built: a bundle is never silently
    /// dropped or granted.
    #[error("tool bundle '{bundle}' is not registered on this mob; the host no longer provides it")]
    ToolBundleUnavailable { bundle: String },

    /// An internal error (unexpected state, logic errors).
    #[error("internal error: {0}")]
    Internal(String),

    /// Publishing a comms participant name was refused because a *different*
    /// live public key already holds that route.
    ///
    /// `meerkat_comms` owns this fact as
    /// [`meerkat_comms::RegistrationRejection::NameOccupied`]; this variant
    /// re-carries the owner's `holder_pubkey` verbatim instead of flattening
    /// the refusal into `Internal(String)` and forcing embedders to match
    /// prose. Construct it only from `comms_name_occupancy_holder` (crate
    /// private), never by inference from an error's display text.
    ///
    /// Deliberate non-mappings: this variant has no
    /// [`Self::wire_detail`]/[`Self::wire_error_code`] projection and no
    /// [`Self::bridge_rejection_cause`]. There is no wire code today that
    /// means "participant name held by a live incumbent", and minting one is
    /// a `meerkat-contracts` change (schema regen + SDK codegen + version
    /// parity), so surfaces keep their existing rendering with the remedy in
    /// the message rather than getting a fabricated code.
    #[error(
        "{}",
        participant_name_occupied_message(participant_name, holder_pubkey)
    )]
    ParticipantNameOccupied {
        participant_name: String,
        holder_pubkey: meerkat_comms::PubKey,
    },

    /// A durable session belongs to the same mob member but carries a
    /// different role, and this exact resume request supplied no migration
    /// declaration. Role remains part of durable comms identity; callers must
    /// opt into the one-way restamp explicitly.
    #[error(
        "durable member '{member_id}' has predecessor role '{stored_role}', but resume targets role '{requested_role}'; supply resume_from_role='{stored_role}' on this exact resume request"
    )]
    MemberRoleMigrationRequired {
        member_id: AgentIdentity,
        stored_role: ProfileName,
        requested_role: ProfileName,
    },

    /// A resume request supplied a role-migration declaration, but the
    /// durable identity facts did not prove the exact declared predecessor.
    #[error(
        "role migration for durable member '{member_id}' from '{declared_predecessor_role}' to '{requested_role}' was rejected: {reason}"
    )]
    MemberRoleMigrationRejected {
        member_id: AgentIdentity,
        declared_predecessor_role: ProfileName,
        requested_role: ProfileName,
        reason: String,
    },

    /// A delivery identity's correlation and a caller-supplied transcript
    /// interaction id (on the `WorkSpec` or in the turn options) name
    /// different interactions. The delivery is refused before admission.
    #[error(
        "delivery correlation '{correlation_id}' conflicts with supplied interaction id '{interaction_id}'"
    )]
    DeliveryInteractionConflict {
        correlation_id: String,
        interaction_id: String,
    },

    /// The mob is shutting down: its Shutdown is draining the mob's own work
    /// and answers this request without running it. (A Shutdown parked on its
    /// members' stops or on its runtime teardown refuses member work this way
    /// too, but no request reaches that refusal today.) Retry once the
    /// Shutdown has finished (the actor then exits, and requests get
    /// [`Self::ActorCommandChannelClosed`]) or failed (the actor stays and
    /// serves the retry). It never means the actor is gone.
    #[error("mob {mob_id} is shutting down; retry once the shutdown has finished or failed")]
    ShutdownInProgress { mob_id: MobId },

    /// A mob create names, in the `tools.bundles` opt-in of `profile`, a
    /// tool bundle the host does not offer to that create. Only a child mob's
    /// inline profiles may opt in, and only to the bundles the host offers to
    /// child mobs (`offered`, empty for any other create). Nothing of the mob
    /// exists yet.
    #[error(
        "profile '{profile}' opts into tool bundle '{bundle}', which is not offered here; offered bundles: [{}]",
        offered.join(", ")
    )]
    ToolBundleNotOffered {
        profile: ProfileName,
        bundle: String,
        offered: Vec<String>,
    },

    /// A durable fork of `source_member_id` was refused before anything was
    /// committed: its child would inherit activated host instructions that
    /// the host's [`meerkat_core::ForkInstructionPolicy`] does not carry
    /// into it. Member forks, `fork_off`, the bounded and turn-boundary
    /// forks and council participants (local or placed) all report it.
    #[error("fork of member '{source_member_id}' refused: {refusal}")]
    ForkInstructionRefused {
        source_member_id: AgentIdentity,
        refusal: meerkat_core::ForkInstructionRefusal,
    },

    /// A prompt-context fork of `source_member_id` was refused before
    /// anything was spawned: the newest row of its planned context does not
    /// fit the fork context budget for the child's model.
    #[error(
        "fork of member '{source_member_id}' refused: its newest context row with the \
         fork context framing needs {required_bytes} bytes, more than the \
         {max_bytes}-byte fork context budget; fork with last_messages or from a \
         source with shorter rows"
    )]
    ForkContextTooLarge {
        source_member_id: AgentIdentity,
        max_bytes: usize,
        /// The bytes the newest row needs together with the fork context
        /// framing (header, fork notice and end marker).
        required_bytes: usize,
    },

    /// A placed spawn that asks its member host `host_id` what it serves (a
    /// prompt-context fork child, or a plain resume) was not spawned: the
    /// `HostFeatures` probe failed with `cause` (a timeout, the transport,
    /// admission). Nothing was recorded for the member. A host that does
    /// not know the probe is not a failure: a fork child gets the context
    /// as a prefix of its first prompt, and a resume keeps its first turn.
    /// It is classified as `cause` everywhere: the failure class, the wire
    /// code and detail, the rejection cause and the structured data are the
    /// cause's. A spawn-time probe holds no durable resume and refuses no
    /// runtime effect.
    #[error("HostFeatures probe of member host '{host_id}' failed: {cause}")]
    HostFeaturesProbeFailed {
        host_id: String,
        cause: Arc<MobError>,
    },

    /// A restored member's host refused its registration at `call`. The
    /// refused registration is gone: removed exactly through the canonical
    /// unregister when the revival did not insert it, so the inputs its
    /// session held end Stopped. The member stays Broken until an explicit
    /// repair, and asking for it again returns this refusal without asking
    /// the host.
    #[error("the host refused the restored registration of member {member_id} at {call}: {reason}")]
    MemberRegistrationRefused {
        member_id: AgentIdentity,
        session_id: Option<meerkat_core::types::SessionId>,
        call: MemberRegistrationHookCall,
        reason: String,
    },

    /// The member's runtime did not install, on the member's registration,
    /// the destination its host selected. This is the runtime's answer, not
    /// a host refusal: the cleanup of a refused restored registration never
    /// settles it, so a restored member keeps its registration for a repair
    /// that selects an advanced context generation. `error` is the runtime's
    /// own verdict, unchanged: a selection whose context generation does not
    /// advance past the one the registration is already bound at is
    /// `RuntimeDriverError::ScopedContextGenerationNotAdvanced`.
    #[error("member session {session_id} did not install its selected destination: {error}")]
    MemberDestinationNotInstalled {
        session_id: meerkat_core::types::SessionId,
        error: meerkat_runtime::RuntimeDriverError,
    },

    /// The mob's store holds a creation record and an event log that
    /// disagree, so the mob is not resumed (or the write that would make
    /// them disagree is refused). The store is left as it was.
    #[error("mob '{mob_id}' has an inconsistent creation record: {inconsistency}")]
    IncarnationInconsistent {
        mob_id: MobId,
        inconsistency: IncarnationInconsistency,
    },

    /// The mob's store holds a finished destroy of `incarnation` (its
    /// creation record is finalized) whose files are not removed yet. The
    /// mob is not resumed.
    #[error("mob '{mob_id}' at incarnation {incarnation} was destroyed")]
    IncarnationDestroyed {
        mob_id: MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
    },

    /// A create by a caller session was refused because the hosted rosters
    /// that could name that session could not all be read, so its provenance
    /// is not known (see `meerkat_mob::MobProvenance`). `mob_id` names the
    /// mob whose roster or creation record could not be read; it is `None`
    /// while the host's mob set is not restored. Nothing was created; the
    /// create can be retried once the named mob is readable (or destroyed)
    /// and the mob set is restored.
    #[error("{}", provenance_unavailable_message(mob_id.as_ref()))]
    ProvenanceUnavailable { mob_id: Option<MobId> },

    /// A member delivery scope captured at another incarnation of this mob
    /// reached a scoped verb (see `meerkat_mob::MemberDeliveryScope`).
    /// `expected` is the incarnation the scope names, `None` for a version-1
    /// scope, which names none and is honoured only by an incarnation
    /// assigned to a store from before 0.9.4. `current` is the incarnation
    /// this mob runs at. Nothing was read or changed.
    #[error("{}", incarnation_mismatch_message(mob_id, expected.as_ref(), current.as_ref()))]
    IncarnationMismatch {
        mob_id: MobId,
        expected: Option<meerkat_core::service::MobIncarnationId>,
        current: Option<meerkat_core::service::MobIncarnationId>,
    },

    /// A respawn was refused before anything was retired: the restrictions
    /// the member inherited from its spawner (launch tool access policy and
    /// visibility ceiling) could not be established for its successor, or
    /// the member changed while they were read. The member keeps running.
    #[error("respawn of member {agent_identity} refused: {reason}")]
    RespawnPredecessorRestrictionsUnavailable {
        agent_identity: String,
        reason: String,
    },

    /// A member retirement's durability reload of the member session was
    /// refused by the runtime as repair-blocked
    /// (`RuntimeDriverError::RecoveryRepairBlocked`), for example while a
    /// boundary commit past its deadline has not reported its durable
    /// outcome. Retryable: once the runtime's evidence settles, a redrive of
    /// the retirement reloads the member and continues. A retirement's
    /// ingress detach that the runtime refuses as repair-blocked, because the
    /// member session degraded after the stage's reload check, answers it
    /// too, with the refusal's own fields. The retire path answers it,
    /// usually as the cause of a stuck retirement;
    /// `MobHandle::reload_member_registration` answers
    /// [`MobError::MemberReloadRefused`] for the same runtime refusal, and a
    /// caller treats both alike. The fields mirror the destroy report's
    /// `MemberReloadRefusal::RepairBlocked`.
    #[error("member session {session_id} reload is repair-blocked: {reason}")]
    MemberReloadRepairBlocked {
        session_id: meerkat_core::types::SessionId,
        reason: String,
        evidence_digest: Option<String>,
    },

    /// A member's operator authority ceiling
    /// ([`crate::MemberOperatorAuthorityCeiling`]) was refused. Nothing was
    /// spawned; a refused respawn retired nothing and the member keeps
    /// running.
    #[error("operator authority ceiling for member {agent_identity} refused: {reason}")]
    OperatorAuthorityCeilingRefused {
        agent_identity: AgentIdentity,
        reason: OperatorAuthorityCeilingRefusal,
    },

    /// A keyed member send (a member send with a caller idempotency key) was
    /// refused, or its answer could not be made; nothing new was committed.
    #[error("keyed send to member '{identity}': {refusal}")]
    MemberSendIdempotency {
        identity: AgentIdentity,
        refusal: MemberSendIdempotencyRefusal,
    },

    /// A graceful Shutdown kept the member input durably queued: it runs on
    /// the mob's next start. It is not failed. Do not resubmit it; a
    /// same-key replay after the restart re-attaches to it.
    #[error(
        "input {input_id} in session {session_id} stays queued across the stop and runs on the next start; re-attach with the same delivery key, do not resubmit"
    )]
    MemberInputRequeuedAcrossStop {
        session_id: meerkat_core::SessionId,
        input_id: meerkat_core::InputId,
    },

    /// An input a graceful Shutdown kept queued was older than its requeue
    /// bound when the mob's next start recovered it, so it ended without
    /// running. Resubmitting it is now correct.
    #[error(
        "input {input_id} in session {session_id} was requeued across a stop but expired before it ran; resubmit it"
    )]
    MemberInputRequeueExpired {
        session_id: meerkat_core::SessionId,
        input_id: meerkat_core::InputId,
    },

    /// The mob's startup could not restore `restore` for member `member_id`'s
    /// session `session_id` (for example while another runtime owner on the
    /// realm hosts the session), so it deferred that restore instead of
    /// failing the whole start, and an operation that acts on the member
    /// re-attempted it first. `cause` is the typed failure of that
    /// re-attempt, and every typed classifier answers as for the cause.
    /// Nothing acted on the member's session. The mob re-attempts the restore
    /// on its own backoff, and the operation can pass once the cause clears
    /// (for a session another runtime owner hosts, once that owner stops or
    /// releases it).
    #[error(
        "member {member_id} session {session_id} has its {restore} deferred until it can be restored: {cause}"
    )]
    MemberCustodyDeferred {
        member_id: AgentIdentity,
        session_id: meerkat_core::types::SessionId,
        restore: MemberCustodyRestore,
        cause: Arc<MobError>,
    },

    /// A spawn of member `member_id` from profile `profile`, which declares
    /// `durable = true`, on a mob whose member sessions do not outlive the
    /// process ([`crate::MobSessionService::keeps_durable_member_sessions`]).
    /// A durable member must survive a restart on its session, so the spawn
    /// is refused before anything is created. Run the mob on a persistent
    /// realm, or drop `durable` from the profile.
    #[error(
        "member {member_id} uses durable profile '{profile}', but the mob's member sessions do not outlive the process: run the mob on a persistent realm, or drop `durable` from the profile"
    )]
    NonDurableService {
        member_id: AgentIdentity,
        profile: ProfileName,
    },
}

/// Why a member's operator authority ceiling was refused
/// ([`MobError::OperatorAuthorityCeilingRefused`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperatorAuthorityCeilingRefusal {
    /// The ceiling narrows the spawn profiles and keeps manage scope. Manage
    /// scope admits every profile, so that narrowing could not hold: narrow
    /// the spawn profiles without manage scope.
    SpawnProfilesWithManage,
    /// The spawn has no placement. A ceiling bounds the authority the
    /// controlling host records for a placed member; a local member's
    /// operator authority is set by its own session build.
    UnplacedMember,
}

impl std::fmt::Display for OperatorAuthorityCeilingRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnProfilesWithManage => f.write_str(
                "it narrows the spawn profiles but keeps manage scope, which admits every \
                 profile; narrow the spawn profiles without manage scope",
            ),
            Self::UnplacedMember => f.write_str(
                "the member is not placed on a member host; a ceiling bounds only a placed \
                 member's recorded authority",
            ),
        }
    }
}

/// Why a keyed member send was refused or could not be answered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberSendIdempotencyRefusal {
    /// The key is not canonical key text.
    InvalidKey { detail: String },
    /// The key is bound to other content; the first delivery stands.
    Conflict {
        delivery_id: String,
        committed_at_ms: u64,
    },
    /// No continuation service is bound on this host, so it cannot keep the
    /// key for the member identity.
    NoContinuationService,
    /// The runtime admits work through a native work authorization host,
    /// which refuses continuations.
    GovernedRuntime,
    /// The continuation authority cannot be read now. The commit, if any,
    /// stands: retry the same key.
    Unavailable { detail: String },
    /// The continuation authority holds facts that cannot all be true.
    Inconsistent { detail: String },
}

impl std::fmt::Display for MemberSendIdempotencyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKey { detail } => f.write_str(detail),
            Self::Conflict { delivery_id, .. } => write!(
                f,
                "the idempotency key is bound to other content (delivery {delivery_id})"
            ),
            Self::NoContinuationService => {
                f.write_str("this host binds no continuation service to keep the key")
            }
            Self::GovernedRuntime => f.write_str("a governed runtime refuses keyed member sends"),
            Self::Unavailable { detail } => {
                write!(f, "continuation status unavailable: {detail}")
            }
            Self::Inconsistent { detail } => {
                write!(f, "continuation authority is inconsistent: {detail}")
            }
        }
    }
}

impl MemberSendIdempotencyRefusal {
    /// The typed wire kind of this refusal.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidKey { .. } => "invalid_idempotency_key",
            Self::Conflict { .. } => "input_idempotency_conflict",
            Self::NoContinuationService | Self::GovernedRuntime => "idempotency_unavailable",
            Self::Unavailable { .. } => "continuation_status_unavailable",
            Self::Inconsistent { .. } => "continuation_status_inconsistent",
        }
    }

    fn structured_data(&self) -> serde_json::Value {
        match self {
            Self::Conflict {
                delivery_id,
                committed_at_ms,
            } => serde_json::json!({
                "kind": self.kind(),
                "delivery_id": delivery_id,
                "committed_at_ms": committed_at_ms,
            }),
            Self::NoContinuationService => serde_json::json!({
                "kind": self.kind(),
                "cause": "no_continuation_service",
            }),
            Self::GovernedRuntime => serde_json::json!({
                "kind": self.kind(),
                "cause": "governed_runtime",
            }),
            Self::InvalidKey { .. } | Self::Unavailable { .. } | Self::Inconsistent { .. } => {
                serde_json::json!({ "kind": self.kind() })
            }
        }
    }

    /// The wire class: an invalid key and a conflict stay invalid params; an
    /// unavailable door is a capability gap; an unreadable status is
    /// retryable; an inconsistent one is internal and not retryable.
    fn error_code(&self) -> Option<meerkat_contracts::ErrorCode> {
        match self {
            Self::InvalidKey { .. } | Self::Conflict { .. } => None,
            Self::NoContinuationService | Self::GovernedRuntime => {
                Some(meerkat_contracts::ErrorCode::CapabilityUnavailable)
            }
            Self::Unavailable { .. } => {
                Some(meerkat_contracts::ErrorCode::SessionRuntimeUnavailable)
            }
            Self::Inconsistent { .. } => Some(meerkat_contracts::ErrorCode::InternalError),
        }
    }
}

/// Which startup restore of a member's custody was deferred (see
/// [`MobError::MemberCustodyDeferred`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberCustodyRestore {
    /// The member's operation binding, restored for a member whose restore
    /// failed at this start.
    StartupBinding,
    /// A retiring member's local cleanup delivery custody.
    CleanupDelivery,
}

impl MemberCustodyRestore {
    /// Stable token in structured data.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StartupBinding => "startup_binding",
            Self::CleanupDelivery => "cleanup_delivery",
        }
    }
}

impl std::fmt::Display for MemberCustodyRestore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StartupBinding => "operation binding restore",
            Self::CleanupDelivery => "cleanup delivery restore",
        })
    }
}

/// The cause a member's deferred custody restore last met: the reason it
/// stays deferred. Only these causes defer a restore; any other failure
/// fails the operation that re-attempted it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberCustodyDeferralCause {
    /// Another runtime owner on the realm hosts the member's session.
    ServedElsewhere,
    /// The member session's hosting claim cannot be taken on this realm.
    HostingUnavailable,
    /// The member's durable tail could not be proven, and its recovery kept
    /// it ([`MobError::MemberDurableTailUnprovable`]).
    DurableTailUnprovable,
}

impl MemberCustodyDeferralCause {
    /// The deferral cause `error` is, if a failed custody restore may be
    /// deferred for it: the typed hosting refusals and the unprovable durable
    /// tail. `None` for any other failure.
    #[must_use]
    pub fn of(error: &MobError) -> Option<Self> {
        match error {
            MobError::SessionError(meerkat_core::SessionError::ServedElsewhere { .. }) => {
                Some(Self::ServedElsewhere)
            }
            MobError::SessionError(meerkat_core::SessionError::HostingUnavailable { .. }) => {
                Some(Self::HostingUnavailable)
            }
            MobError::MemberDurableTailUnprovable { .. } => Some(Self::DurableTailUnprovable),
            _ => None,
        }
    }

    /// Stable token in structured data and on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ServedElsewhere => "served_elsewhere",
            Self::HostingUnavailable => "hosting_unavailable",
            Self::DurableTailUnprovable => "durable_tail_unprovable",
        }
    }
}

/// A member's custody restore a mob start deferred instead of failing the
/// whole start, as the member's status shows it: which restore, and the cause
/// its last attempt met. Every operation on the member re-attempts the
/// restore first, and the deferral clears once one succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct MemberCustodyDeferral {
    /// The deferred restore.
    pub restore: MemberCustodyRestore,
    /// The cause its last attempt met.
    pub cause: MemberCustodyDeferralCause,
}

impl MemberCustodyDeferral {
    /// The deferral of `restore` whose last attempt met `cause`, when the
    /// cause may defer a restore.
    #[must_use]
    pub fn new(restore: MemberCustodyRestore, cause: &MobError) -> Option<Self> {
        MemberCustodyDeferralCause::of(cause).map(|cause| Self { restore, cause })
    }
}

/// The stable token of the runtime verdict a member's destination install
/// got, for its wire data: only `scoped_context_generation_not_advanced` is
/// repaired by selecting an advanced context generation.
fn destination_install_refusal_cause(error: &meerkat_runtime::RuntimeDriverError) -> &'static str {
    use meerkat_runtime::RuntimeDriverError as Verdict;
    match error {
        Verdict::ScopedContextGenerationNotAdvanced { .. } => {
            "scoped_context_generation_not_advanced"
        }
        Verdict::ValidationFailed { .. } => "validation_failed",
        Verdict::StaleAuthority { .. } => "stale_authority",
        Verdict::NotFound { .. } => "not_found",
        Verdict::NotReady { .. } => "not_ready",
        Verdict::Destroyed => "destroyed",
        Verdict::Internal(_) => "internal",
        _ => "other",
    }
}

fn incarnation_mismatch_message(
    mob_id: &MobId,
    expected: Option<&meerkat_core::service::MobIncarnationId>,
    current: Option<&meerkat_core::service::MobIncarnationId>,
) -> String {
    let expected = match expected {
        Some(incarnation) => format!("incarnation {incarnation}"),
        None => "no incarnation (a version-1 scope)".to_string(),
    };
    let current = match current {
        Some(incarnation) => format!("runs at incarnation {incarnation}"),
        None => "has no live incarnation here".to_string(),
    };
    format!("mob '{mob_id}' {current}; the delivery scope names {expected}")
}

fn provenance_unavailable_message(mob_id: Option<&MobId>) -> String {
    match mob_id {
        Some(mob_id) => format!(
            "the creator of this mob cannot be recorded: the roster of mob '{mob_id}' could not \
             be read"
        ),
        None => "the creator of this mob cannot be recorded: the host's mob set is not restored \
                 yet"
        .to_string(),
    }
}

/// THE single owner of the operator-facing name-occupancy remedy text, shared
/// by every mob-layer `ParticipantNameOccupied` variant so the three surfaces
/// cannot drift into three different instructions.
///
/// Remedy first, evidence second, and an explicit disclaimer of the reading
/// that cost an adopter an hour in 0.8.23: a refused claim that names a public
/// key reads as a crypto/trust failure when it is almost always a second
/// runtime under the same participant name in one process.
///
/// 0.8.24 made this the ONLY claim rule, so the newly refused - and now
/// dominant - shape is a host rebuilding a mob whose predecessor is still live,
/// under the SAME supervisor authority key. That case used to succeed silently
/// by displacing the predecessor's route, so the message has to say what the
/// operator must do about their own predecessor, not merely that a claim was
/// refused.
pub(crate) fn participant_name_occupied_message(
    participant_name: &str,
    holder_pubkey: &meerkat_comms::PubKey,
) -> String {
    format!(
        "participant name '{participant_name}' already has a live route: the incumbent has not \
         released this route, so retire the incumbent first - shut the predecessor down \
         (MobHandle::shutdown) or drop its runtime - and then publish this name again. This \
         applies even when the incumbent is an earlier host of THIS mob holding THIS same \
         authority key: holding the key is not a claim on the route, and succession is admitted \
         only on evidence (the incumbent released the name, or the predecessor generation was \
         handed in). The live route is held by public key {} - that is evidence of who holds the \
         name, not a key or trust failure",
        holder_pubkey.to_pubkey_string()
    )
}

/// Re-carry [`meerkat_comms::RegistrationRejection::NameOccupied`]'s holder key
/// out of a comms construction/publication failure.
///
/// This is the ONLY seam that reads the comms-owned rejection at the mob layer.
/// Every mob-layer conversion of a `CommsRuntimeError` routes through it, so
/// the knowledge of the rejection's shape lives in exactly one place and no
/// call site re-derives occupancy from display text.
pub(crate) fn comms_name_occupancy_holder(
    error: &meerkat_comms::CommsRuntimeError,
) -> Option<meerkat_comms::PubKey> {
    // `RegistrationRejection` is `#[non_exhaustive]`: an `if let` states that
    // this seam recognises exactly one shape, where a wildcard match arm would
    // look total and silently absorb a future rejection kind.
    if let meerkat_comms::CommsRuntimeError::InprocRegistrationRejected(
        meerkat_comms::RegistrationRejection::NameOccupied { holder_pubkey },
    ) = error
    {
        return Some(*holder_pubkey);
    }
    None
}

/// Why `ClassifyFlowStepDispatch` refused a flow-step dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStepDispatchRejectKind {
    /// Overlay-bearing steps cannot target autonomous-host members — the
    /// injector admission seam carries no overlay, on ANY host (gotcha 12).
    OverlayAutonomous,
    /// The member's host cannot serve tracked turns (`durable_sessions =
    /// false`: no durable turn-outcome journal).
    HostIncapable,
}

impl std::fmt::Display for FlowStepDispatchRejectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OverlayAutonomous => f.write_str(
                "turn tool overlay cannot be enforced for an autonomous-host member \
                 (overlay_autonomous)",
            ),
            Self::HostIncapable => f.write_str(
                "member host cannot serve tracked flow turns (host_incapable: \
                 durable_sessions=false)",
            ),
        }
    }
}

/// Why a fork source member's conversation history cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkSourceUnavailableCause {
    /// The source member has no session (peer-only external source —
    /// unchanged semantics, now typed).
    NoSession,
    /// The source owes work: a turn in flight, an admitted input that has
    /// not started, or a committed turn not yet answered. The persistent
    /// fork owner refuses to observe and copy it concurrently.
    Running,
    /// The source is placed on a member host; the proxied fork-context
    /// history read lands in phase 6 (§19.L2). Typed from birth so the
    /// phase-3→6 window never ships the untyped shape.
    RemoteReadUnavailable,
}

impl std::fmt::Display for ForkSourceUnavailableCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => f.write_str("source member has no session"),
            Self::Running => f.write_str("source member session is running"),
            Self::RemoteReadUnavailable => f.write_str(
                "source member is placed on a member host; remote history reads are unavailable",
            ),
        }
    }
}

fn held_cleanup_suffix(cleanup: Option<&MobError>) -> String {
    match cleanup {
        None => String::new(),
        Some(error) => format!("; its abort did not confirm cleanup: {error}"),
    }
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_identities(identities: &[AgentIdentity]) -> String {
    identities
        .iter()
        .map(AgentIdentity::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_member_restore_target(session_id: Option<&meerkat_core::types::SessionId>) -> String {
    match session_id {
        Some(session_id) => format!("session {session_id}"),
        None => "runtime bridge state".to_string(),
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for MobError {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::StorageError(error)
    }
}

impl From<crate::store::MobStoreError> for MobError {
    fn from(error: crate::store::MobStoreError) -> Self {
        match error {
            crate::store::MobStoreError::SpecRevisionConflict {
                mob_id,
                expected,
                actual,
            } => Self::SpecRevisionConflict {
                mob_id,
                expected,
                actual,
            },
            crate::store::MobStoreError::MobDefinitionProjectionMismatch {
                mob_id,
                authority_epoch,
                projection_revision,
                kind,
            } => Self::MobDefinitionProjectionMismatch {
                mob_id,
                authority_epoch,
                projection_revision,
                kind,
            },
            crate::store::MobStoreError::FrameAtomicPersistenceUnavailable { operation } => {
                Self::FrameAtomicPersistenceUnavailable { operation }
            }
            crate::store::MobStoreError::IncarnationInconsistent {
                mob_id,
                inconsistency,
            } => Self::IncarnationInconsistent {
                mob_id,
                inconsistency,
            },
            other => Self::StorageError(Box::new(other)),
        }
    }
}

impl From<ScopeDenial> for MobError {
    fn from(denial: ScopeDenial) -> Self {
        Self::ScopeDenied(denial)
    }
}

impl From<BridgeRejectionReply> for MobError {
    fn from(rejection: BridgeRejectionReply) -> Self {
        let cause = rejection.typed_cause();
        let reason = rejection.reason().to_string();
        match cause {
            Some(cause) => Self::BridgeCommandRejected { cause, reason },
            None => Self::WiringError(reason),
        }
    }
}

impl MobError {
    /// Structured-data `kind` of [`Self::held_before_activation`].
    pub const HELD_BEFORE_ACTIVATION_KIND: &'static str = "mob_held_before_activation";

    /// A persisted mob its host holds before activation: restore has not
    /// started it, because the owner session it is bound to is unresolved
    /// (`reason`, a host-defined token). Projected under the
    /// runtime-unavailable class, never as a missing mob.
    ///
    /// A one-way report only. The host decides activation, destroy admission
    /// and retry from its own typed hold and the mob's durable history,
    /// never from this projection. `content_retained` and `retryable` are
    /// what the lookup observed (the store was kept; asking again may
    /// answer differently), not cleanup or lease guarantees.
    #[doc(hidden)]
    pub fn held_before_activation(
        mob_id: &MobId,
        owner: &meerkat_core::types::SessionId,
        reason: &'static str,
    ) -> Self {
        Self::SessionError(meerkat_core::service::SessionError::FailedWithData {
            message: format!(
                "mob {mob_id} is held before activation: its owner session {owner} is unresolved ({reason})"
            ),
            data: serde_json::json!({
                "kind": Self::HELD_BEFORE_ACTIVATION_KIND,
                "code": "SESSION_RUNTIME_UNAVAILABLE",
                "mob_id": mob_id.to_string(),
                "owner_session_id": owner.to_string(),
                "reason": reason,
                "content_retained": true,
                "retryable": true,
            }),
        })
    }

    /// Whether this is a [`Self::held_before_activation`] answer.
    pub fn is_held_before_activation(&self) -> bool {
        match self {
            Self::SessionError(meerkat_core::service::SessionError::FailedWithData {
                data,
                ..
            }) => {
                data.get("kind").and_then(serde_json::Value::as_str)
                    == Some(Self::HELD_BEFORE_ACTIVATION_KIND)
            }
            _ => false,
        }
    }

    /// Structured-data `kind` of [`Self::MemberReloadRepairBlocked`].
    pub(crate) const MEMBER_RELOAD_REPAIR_BLOCKED_KIND: &'static str =
        "mob_member_reload_repair_blocked";

    /// The structured data of [`Self::MemberReloadRepairBlocked`]. A
    /// retirement stage carries the refusal across its `SessionError`
    /// boundary as this data, and the retirement maps it back to the
    /// variant.
    pub(crate) fn member_reload_repair_blocked_data(
        session_id: &meerkat_core::types::SessionId,
        reason: &str,
        evidence_digest: Option<&str>,
    ) -> serde_json::Value {
        let mut data = serde_json::json!({
            "kind": Self::MEMBER_RELOAD_REPAIR_BLOCKED_KIND,
            "session_id": session_id.to_string(),
            "reason": reason,
            "retryable": true,
            "authority_retained": true,
            "live_shell_discarded": false,
        });
        if let (Some(digest), Some(fields)) = (evidence_digest, data.as_object_mut()) {
            fields.insert("evidence_digest".to_string(), digest.into());
        }
        data
    }

    /// Structured data carried by an underlying domain error.
    ///
    /// Session materialization failures use this to preserve typed
    /// provider-auth causes through Meta and RPC without teaching the mob
    /// layer to reclassify authentication itself.
    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::MemberSendIdempotency { refusal, .. } => Some(refusal.structured_data()),
            Self::FlowRoleTargetsRefused { flow_id, refusal } => Some(serde_json::json!({
                "kind": "flow_role_targets_refused",
                "flow_id": flow_id.as_str(),
                "refusal": refusal,
            })),
            Self::BoundFlowTargetsUnavailable {
                run_id,
                step_id,
                required,
                available,
                unavailable,
            } => Some(serde_json::json!({
                "kind": "bound_flow_targets_unavailable",
                "run_id": run_id.to_string(),
                "step_id": step_id.as_str(),
                "required": required,
                "available": available,
                "unavailable": unavailable,
            })),
            Self::FlowRunIdempotencyConflict { run_id, flow_id } => Some(serde_json::json!({
                "kind": "flow_run_idempotency_conflict",
                "run_id": run_id.to_string(),
                "flow_id": flow_id.as_str(),
            })),
            Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                error.structured_data()
            }
            // A failed probe is classified by what failed it.
            Self::HostFeaturesProbeFailed { cause, .. } => cause.structured_data(),
            Self::SessionError(error) => meerkat_contracts::error::session_error_details(error),
            Self::ForkInstructionRefused {
                source_member_id,
                refusal,
            } => Some(serde_json::json!({
                "kind": meerkat_core::service::SessionError::FORK_INSTRUCTION_REFUSED_KIND,
                "fork_instruction_refusal": refusal,
                "source_member_id": source_member_id.as_str(),
            })),
            Self::ForkContextTooLarge {
                source_member_id,
                max_bytes,
                required_bytes,
            } => Some(serde_json::json!({
                "kind": "fork_context_too_large",
                "source_member_id": source_member_id.as_str(),
                "max_bytes": max_bytes,
                "required_bytes": required_bytes,
            })),
            // A restore failure that IS a typed resume hold projects the hold
            // under meerkat's own key, so a wire caller classifies the Broken
            // member exactly as it classifies meerkat's -32013 refusal.
            Self::MemberRestoreFailed {
                session_id,
                hold: Some(hold),
                ..
            } => Some(serde_json::json!({
                "kind": "mob_member_restore_failed",
                meerkat_core::service::SessionError::DURABLE_RESUME_HOLD_KEY: hold.as_str(),
                "session_id": session_id.as_ref().map(ToString::to_string),
                "content_retained": true,
                "retryable": false,
            })),
            // The member and its session are this variant's own facts; the
            // cause keeps its structured data under "cause_data", and its
            // durable resume hold is also lifted to the top level, under
            // meerkat's own key, so a reader of that key classifies the
            // failure exactly as it would the cause alone. Every typed
            // classifier above and below delegates to the cause.
            // The close's reason and the cause's own data. The cause's hold
            // is deliberately NOT lifted to the top-level key, which
            // `durable_resume_hold` reads: a fail-stop parks nothing.
            Self::DurabilityFailStop { mob_id, cause } => Some(serde_json::json!({
                "kind": "mob_durability_fail_stop",
                "mob_id": mob_id.as_str(),
                "reason": "durability_fail_stop",
                "cause": cause.to_string(),
                "cause_data": cause.structured_data(),
                "actor_stopped": true,
            })),
            Self::MemberStartupRecoveryFailed {
                member_id,
                session_id,
                cause,
            } => {
                let mut data = serde_json::json!({
                    "kind": "mob_member_startup_recovery_failed",
                    "member_id": member_id.as_str(),
                    "session_id": session_id.to_string(),
                    "cause": cause.to_string(),
                    "cause_data": cause.structured_data(),
                    "actor_stopped": true,
                });
                if let (Some(hold), Some(object)) =
                    (cause.durable_resume_hold(), data.as_object_mut())
                {
                    object.insert(
                        meerkat_core::service::SessionError::DURABLE_RESUME_HOLD_KEY.to_string(),
                        serde_json::Value::from(hold.as_str()),
                    );
                }
                Some(data)
            }
            Self::MemberDurableTailUnprovable {
                member_id,
                session_id,
            } => Some(serde_json::json!({
                "kind": "mob_member_durable_tail_unprovable",
                "member_id": member_id.as_str(),
                "session_id": session_id.to_string(),
                meerkat_core::service::SessionError::DURABLE_RESUME_HOLD_KEY:
                    meerkat_core::service::DurableResumeHold::TailHeldForRecovery.as_str(),
                "content_retained": true,
                "retryable": false,
            })),
            // As for a startup recovery failure: the member, its session and
            // the deferred restore are this variant's own facts; the cause
            // keeps its structured data under "cause_data" (when it has some)
            // and its durable resume hold is lifted to the top level.
            Self::MemberCustodyDeferred {
                member_id,
                session_id,
                restore,
                cause,
            } => {
                let mut data = serde_json::json!({
                    "kind": "mob_member_custody_deferred",
                    "member_id": member_id.as_str(),
                    "session_id": session_id.to_string(),
                    "restore": restore.as_str(),
                    "cause": cause.to_string(),
                });
                if let Some(object) = data.as_object_mut() {
                    if let Some(cause_data) = cause.structured_data() {
                        object.insert("cause_data".to_string(), cause_data);
                    }
                    if let Some(hold) = cause.durable_resume_hold() {
                        object.insert(
                            meerkat_core::service::SessionError::DURABLE_RESUME_HOLD_KEY
                                .to_string(),
                            serde_json::Value::from(hold.as_str()),
                        );
                    }
                }
                Some(data)
            }
            Self::NonDurableService { member_id, profile } => Some(serde_json::json!({
                "kind": "mob_non_durable_service",
                "member_id": member_id.as_str(),
                "profile": profile.as_str(),
                "retryable": false,
            })),
            Self::MemberRegistrationRefused {
                member_id,
                session_id,
                call,
                ..
            } => Some(serde_json::json!({
                "kind": "mob_member_registration_refused",
                "member_id": member_id.as_str(),
                "session_id": session_id.as_ref().map(ToString::to_string),
                "hook_call": call.as_str(),
                "registration_removed": true,
                "retryable": false,
            })),
            // The runtime's answer, not a host refusal: the registration is
            // kept, and the repair selects again with an advanced generation.
            Self::MemberDestinationNotInstalled { session_id, error } => {
                let generations = match error {
                    meerkat_runtime::RuntimeDriverError::ScopedContextGenerationNotAdvanced {
                        bound,
                        requested,
                    } => Some((*bound, *requested)),
                    _ => None,
                };
                Some(serde_json::json!({
                    "kind": "mob_member_destination_not_installed",
                    "session_id": session_id.to_string(),
                    "cause": destination_install_refusal_cause(error),
                    "bound_generation": generations.map(|(bound, _)| bound),
                    "requested_generation": generations.map(|(_, requested)| requested),
                    "retryable": false,
                }))
            }
            Self::ForkMemberProvisionFailed {
                member_id,
                fork_session_id,
                ..
            } => Some(serde_json::json!({
                "kind": "fork_member_provision_failed",
                "member_id": member_id.as_str(),
                "fork_session_id": fork_session_id.to_string(),
                "recovery": "resume_committed_fork_session",
            })),
            Self::RetirementInProgress { session_id, stage } => Some(serde_json::json!({
                "kind": "mob_retirement_in_progress",
                "session_id": session_id.to_string(),
                "stage": stage,
                "deadline_reached": true,
                "retryable": true,
                "authority_retained": true,
            })),
            Self::MemberRetirementInProgress { member_id, stage } => Some(serde_json::json!({
                "kind": "mob_retirement_in_progress",
                "member_id": member_id.as_str(),
                "stage": stage,
                "deadline_reached": true,
                "retryable": true,
                "authority_retained": true,
            })),
            Self::MemberRetirementStuck {
                member_id,
                stage,
                cause,
            } => {
                let mut data = serde_json::json!({
                    "kind": "mob_retirement_stuck",
                    "member_id": member_id.as_str(),
                    "stage": stage,
                    "cause": cause.to_string(),
                    "retryable": false,
                    "redrive": true,
                    "authority_retained": true,
                });
                // The cause's own data, so a client routes on its kind
                // rather than on the cause's text.
                if let (Some(cause_data), Some(fields)) =
                    (cause.structured_data(), data.as_object_mut())
                {
                    fields.insert("cause_data".to_string(), cause_data);
                }
                Some(data)
            }
            Self::MemberRetirementAdmissionPending { member_id, stage } => {
                Some(serde_json::json!({
                    "kind": "mob_retirement_admission_pending",
                    "member_id": member_id.as_str(),
                    "stage": stage,
                    "deadline_reached": true,
                    "retryable": true,
                    "authority_retained": false,
                }))
            }
            Self::LifecycleOperationPending { intent } => Some(serde_json::json!({
                "kind": "mob_lifecycle_operation_pending",
                "intent": intent,
                "retryable": true,
                "authority_retained": true,
            })),
            // No deadline field: the refusal claims none. The Shutdown
            // either finishes (the actor exits) or fails (the actor stays).
            Self::ShutdownInProgress { mob_id } => Some(serde_json::json!({
                "kind": "mob_shutdown_in_progress",
                "mob_id": mob_id.as_str(),
                "retryable": true,
                "authority_retained": true,
            })),
            Self::LifecycleOperationProgressStalled {
                intent,
                member_id,
                stage,
            } => Some(serde_json::json!({
                "kind": "mob_lifecycle_operation_progress_stalled",
                "intent": intent,
                "member_id": member_id.as_ref().map(AgentIdentity::as_str),
                "stage": stage,
                "retryable": true,
                "authority_retained": true,
            })),
            Self::LifecycleOperationAdmissionPending { intent, stage } => Some(serde_json::json!({
                "kind": "mob_lifecycle_operation_admission_pending",
                "intent": intent,
                "stage": stage,
                "deadline_reached": true,
                "retryable": true,
                "authority_retained": false,
            })),
            Self::MemberReloadRequired { member_id, reason } => Some(serde_json::json!({
                "kind": "mob_member_reload_required",
                "member_id": member_id.as_str(),
                "reason": reason,
                "retryable": false,
                "authority_retained": true,
                "required_action": "reload_member_registration",
            })),
            Self::MemberRuntimeDetached {
                session_id,
                detachment,
            } => Some(serde_json::json!({
                "kind": "mob_member_runtime_detached",
                "session_id": session_id.to_string(),
                "detachment": detachment.as_str(),
                "retryable": false,
                "authority_retained": true,
                "required_action": "reload_member_registration",
            })),
            Self::MemberAdmissionBacklogFull { member_id, depth } => Some(serde_json::json!({
                "kind": "mob_member_admission_backlog_full",
                "member_id": member_id.as_str(),
                "depth": depth,
                "retryable": true,
                "authority_retained": true,
            })),
            Self::ActorCommandTimedOut {
                command_kind,
                stage,
            } => Some(serde_json::json!({
                "kind": "mob_actor_command_timed_out",
                "command_kind": command_kind,
                "stage": stage,
                "deadline_reached": true,
            })),
            Self::MemberReloadRefused { session_id, reason } => Some(serde_json::json!({
                "kind": "mob_member_reload_refused",
                "session_id": session_id.to_string(),
                "reason": reason,
                "retryable": true,
                "authority_retained": true,
                "live_shell_discarded": false,
            })),
            Self::MemberReloadRepairBlocked {
                session_id,
                reason,
                evidence_digest,
            } => Some(Self::member_reload_repair_blocked_data(
                session_id,
                reason,
                evidence_digest.as_deref(),
            )),
            Self::MemberReloadTimedOut { session_id, stage } => Some(serde_json::json!({
                "kind": "mob_member_reload_timed_out",
                "session_id": session_id.to_string(),
                "stage": stage,
                "deadline_reached": true,
                "retryable": true,
            })),
            Self::SupervisorProtocolUpgradeRequired {
                operation,
                current,
                required,
            } => Some(serde_json::json!({
                "kind": "mob_supervisor_protocol_upgrade_required",
                "operation": operation,
                "current_protocol": current,
                "required_protocol": required,
                "required_action": "rotate_supervisor",
                "retryable": false,
                "authority_retained": true,
            })),
            Self::DirectMemberAdoptionPending {
                current_epoch,
                reason,
            } => Some(serde_json::json!({
                "kind": "mob_direct_member_adoption_pending",
                "current_epoch": current_epoch,
                "reason": reason,
                "retryable": true,
                "authority_retained": true,
                "supervisor_authority_committed": true,
                "required_action": "retry_rotate_supervisor",
            })),
            Self::BridgeCommandRejected {
                cause:
                    BridgeRejectionCause::MaterializeBuildRejected {
                        cause: build_rejection,
                    },
                ..
            } => build_rejection
                .provider_auth_failure()
                .map(|failure| failure.structured_data()),
            _ => None,
        }
    }

    /// Whether a member's custody restore at mob start failed for a cause
    /// that can clear without this owner acting: another runtime owner hosts
    /// the session, its hosting claim is unavailable, or its durable tail is
    /// kept unprovable. Only these causes let the start defer that member
    /// instead of failing; every other cause stays fatal.
    pub(crate) fn is_deferrable_custody_cause(&self) -> bool {
        MemberCustodyDeferralCause::of(self).is_some()
    }

    /// meerkat's typed durable resume hold carried by this error, if any:
    /// the session is intact and withheld (a held tail, a refused recovery,
    /// quarantined evidence, or a WholeBlob document that needs the
    /// sanctioned audited-endpoint repair), not faulted. Read typed off the
    /// in-process `SessionError` carrier, through the shared retirement /
    /// lifecycle failure wrappers a joined observer receives, or off the
    /// structured `durable_resume_hold` wire token; never off display text.
    /// Hosts classify on this instead of parsing prose (Meta parks the
    /// member with the exact repair commands on `AuditedEndpointDivergence`).
    pub fn durable_resume_hold(&self) -> Option<meerkat_core::service::DurableResumeHold> {
        match self {
            Self::SessionError(error) => error.durable_resume_hold(),
            Self::MemberRestoreFailed { hold, .. } => *hold,
            Self::SharedRetirementFailure(error)
            | Self::SharedLifecycleFailure(error)
            | Self::MemberRetirementStuck { cause: error, .. }
            | Self::MemberCustodyDeferred { cause: error, .. }
            | Self::MemberStartupRecoveryFailed { cause: error, .. } => error.durable_resume_hold(),
            // Restart, do not park: the cause's hold belongs to a mutation
            // the stopped actor never retries.
            Self::DurabilityFailStop { .. } => None,
            other => other
                .structured_data()
                .as_ref()
                .and_then(meerkat_core::service::SessionError::durable_resume_hold_from_data),
        }
    }

    /// Canonical public wire status for Mob failures that carry a stable
    /// transport classification. Typed detail remains owned by
    /// [`Self::wire_detail`]; this sibling also covers retryable lifecycle
    /// states whose structured payload is not one of the four multi-host
    /// detail envelopes.
    pub fn wire_error_code(&self) -> Option<meerkat_contracts::ErrorCode> {
        self.wire_detail()
            .map(|detail| detail.code())
            .or_else(|| match self {
                Self::SharedRetirementFailure(error)
                | Self::SharedLifecycleFailure(error)
                | Self::MemberCustodyDeferred { cause: error, .. }
                | Self::MemberStartupRecoveryFailed { cause: error, .. } => error.wire_error_code(),
                Self::HostFeaturesProbeFailed { cause, .. } => cause.wire_error_code(),
                // The close's answer (AdmissionClosed carries no code).
                Self::DurabilityFailStop { .. } => None,
                Self::SessionError(
                    meerkat_core::SessionError::RuntimeUnavailable { .. }
                    | meerkat_core::SessionError::HostingUnavailable { .. },
                ) => Some(meerkat_contracts::ErrorCode::SessionRuntimeUnavailable),
                Self::SessionError(meerkat_core::SessionError::CapabilityUnavailable(_)) => {
                    Some(meerkat_contracts::ErrorCode::CapabilityUnavailable)
                }
                // A member that cannot be scope-bound (no native session
                // binding) does not support scope-bound delivery at all.
                Self::DeliveryScopeUnavailable { .. } => {
                    Some(meerkat_contracts::ErrorCode::CapabilityUnavailable)
                }
                // A durable profile needs persistent sessions, a capability
                // the mob's session service does not have.
                Self::NonDurableService { .. } => {
                    Some(meerkat_contracts::ErrorCode::CapabilityUnavailable)
                }
                error if error.is_held_before_activation() => {
                    Some(meerkat_contracts::ErrorCode::SessionRuntimeUnavailable)
                }
                Self::RetirementInProgress { .. }
                | Self::MemberRetirementInProgress { .. }
                | Self::MemberRetirementAdmissionPending { .. }
                | Self::LifecycleOperationPending { .. }
                | Self::LifecycleOperationProgressStalled { .. }
                | Self::LifecycleOperationAdmissionPending { .. }
                | Self::ShutdownInProgress { .. }
                | Self::DirectMemberAdoptionPending { .. }
                | Self::MemberAdmissionBacklogFull { .. }
                | Self::ActorCommandTimedOut { .. }
                | Self::MemberReloadRefused { .. }
                | Self::MemberReloadRepairBlocked { .. }
                | Self::MemberReloadTimedOut { .. } => {
                    Some(meerkat_contracts::ErrorCode::SessionBusy)
                }
                // Dedicated code (#1105), deliberately NOT `SessionBusy`:
                // the degraded registration never clears on its own, so
                // ordinary busy/backoff retry is never correct here — only
                // the named `reload_member_registration` repair resolves
                // it, mirroring `SupervisorProtocolUpgradeRequired`'s own
                // dedicated non-retryable code below.
                Self::MemberReloadRequired { .. } => {
                    Some(meerkat_contracts::ErrorCode::MemberReloadRequired)
                }
                Self::SupervisorProtocolUpgradeRequired { .. } => {
                    Some(meerkat_contracts::ErrorCode::SupervisorRotationIncomplete)
                }
                Self::ForkInstructionRefused { .. } => {
                    Some(meerkat_contracts::ErrorCode::ForkInstructionRefused)
                }
                Self::ForkContextTooLarge { .. } => {
                    Some(meerkat_contracts::ErrorCode::InvalidParams)
                }
                Self::MemberSendIdempotency { refusal, .. } => refusal.error_code(),
                _ => None,
            })
    }

    pub fn bridge_rejection_cause(&self) -> Option<BridgeRejectionCause> {
        match self {
            Self::SharedRetirementFailure(error)
            | Self::SharedLifecycleFailure(error)
            | Self::MemberRetirementStuck { cause: error, .. }
            | Self::MemberCustodyDeferred { cause: error, .. }
            | Self::MemberStartupRecoveryFailed { cause: error, .. } => {
                error.bridge_rejection_cause()
            }
            Self::HostFeaturesProbeFailed { cause, .. } => cause.bridge_rejection_cause(),
            Self::DurabilityFailStop { .. } => None,
            // Cloned: `BridgeRejectionCause` carries payload variants since V4.
            Self::BridgeCommandRejected { cause, .. } => Some(cause.clone()),
            _ => None,
        }
    }

    /// Whether a failed external provisioning attempt has uncertified
    /// compensation and therefore requires fail-stop handling.
    ///
    /// This typed verdict is consumed by the MobMachine-owning actor when it
    /// decides whether `pending_recipient_trust` may be rolled back. Human
    /// error text is deliberately not part of that decision.
    pub(crate) fn external_member_cleanup_is_uncertain(&self) -> bool {
        matches!(self, Self::ExternalMemberCleanupUncertain { .. })
    }

    /// True only after a consumer refusal has been closed through the
    /// generated producer-feedback input. Actor-loop callers may continue;
    /// operation-scoped callers still return the typed failure to their user.
    pub(crate) fn is_closed_runtime_effect_refusal(&self) -> bool {
        match self {
            Self::RuntimeEffectRefused { .. } => true,
            Self::SharedRetirementFailure(error)
            | Self::SharedLifecycleFailure(error)
            | Self::MemberRetirementStuck { cause: error, .. }
            | Self::MemberCustodyDeferred { cause: error, .. }
            | Self::MemberStartupRecoveryFailed { cause: error, .. } => {
                error.is_closed_runtime_effect_refusal()
            }
            Self::DurabilityFailStop { .. } => false,
            _ => false,
        }
    }

    /// The root cause of a retirement failure: sees through
    /// [`Self::SharedRetirementFailure`] and [`Self::MemberRetirementStuck`]
    /// to the typed error the failing stage produced.
    pub fn retirement_root_cause(&self) -> &MobError {
        match self {
            Self::SharedRetirementFailure(cause) | Self::MemberRetirementStuck { cause, .. } => {
                cause.retirement_root_cause()
            }
            other => other,
        }
    }

    /// This error with [`Self::SharedLifecycleFailure`] peeled: the error a
    /// shared lifecycle failure carries (through nested ones), or this error
    /// itself. A match on a variant that a fail-stop's triggering reply can
    /// arrive wrapped in reads through this.
    pub fn without_shared_lifecycle_wrapper(&self) -> &MobError {
        match self {
            Self::SharedLifecycleFailure(error) => error.without_shared_lifecycle_wrapper(),
            other => other,
        }
    }

    /// This error as a retirement topology cleanup failure: one already is
    /// comes back unchanged, also when it arrives as a fail-stop's shared
    /// reply (the same shared cause, one prefix); any other is wrapped with
    /// its rendering.
    pub(crate) fn into_retirement_topology_incomplete(self) -> MobError {
        match self.without_shared_lifecycle_wrapper() {
            Self::RetirementTopologyIncomplete(_) => self,
            _ => Self::RetirementTopologyIncomplete(self.to_string()),
        }
    }

    /// This error as a wiring failure: one already is comes back unchanged,
    /// also when it arrives as a fail-stop's shared reply (the same shared
    /// cause, one prefix); any other is wrapped with its rendering.
    pub(crate) fn into_wiring_error(self) -> MobError {
        match self.without_shared_lifecycle_wrapper() {
            Self::WiringError(_) => self,
            _ => Self::WiringError(self.to_string()),
        }
    }

    /// Whether this is a typed "retirement still in progress" answer: the
    /// caller's retirement budget elapsed while the process-owned saga keeps
    /// running with its durable anchor retained. Not a failure; retrying the
    /// retirement joins or resumes the same saga. Sees through
    /// [`Self::SharedRetirementFailure`].
    pub fn is_retirement_in_progress(&self) -> bool {
        match self {
            Self::SharedRetirementFailure(error) => error.is_retirement_in_progress(),
            Self::RetirementInProgress { .. } | Self::MemberRetirementInProgress { .. } => true,
            _ => false,
        }
    }

    /// Whether this error means the addressed target (mob, profile, member,
    /// flow, run, or work unit) does not exist.
    ///
    /// Owned here so target-existence probing does not re-`match` the
    /// `MobError` variant list in downstream classifiers.
    pub fn is_missing_target(&self) -> bool {
        matches!(self.failure_class(), MobFailureClass::TargetMissing)
    }

    /// Classify this error into the mob-owned [`MobFailureClass`].
    ///
    /// This is the single source of truth for which `MobError` variants fall
    /// into which failure class; consumers map [`MobFailureClass`] onto their
    /// own domain vocabularies rather than re-matching the variant list.
    pub fn failure_class(&self) -> MobFailureClass {
        match self {
            Self::SharedRetirementFailure(error)
            | Self::SharedLifecycleFailure(error)
            | Self::MemberCustodyDeferred { cause: error, .. }
            | Self::MemberStartupRecoveryFailed { cause: error, .. } => error.failure_class(),
            // A failed probe is classified by what failed it.
            Self::HostFeaturesProbeFailed { cause, .. } => cause.failure_class(),
            // The close's answer: AdmissionClosed falls to MobRejected.
            Self::DurabilityFailStop { .. } => MobFailureClass::MobRejected,
            Self::MobNotFound(_)
            | Self::ProfileNotFound(_)
            | Self::MemberNotFound(_)
            | Self::MemberSessionNotLive { .. }
            | Self::FlowNotFound(_)
            | Self::RunNotFound(_)
            | Self::WorkNotFound(_)
            // The addressed incarnation is over for good: a finished destroy,
            // or a scope captured at an incarnation this mob does not run at
            // (a live handle's `current` is final).
            | Self::IncarnationDestroyed { .. }
            | Self::IncarnationMismatch { .. } => MobFailureClass::TargetMissing,
            // The same verdict as a resume refused at an unprovable committed
            // boundary: the tail stays until its recovery or an operator
            // resolves it, never a plain busy/backoff retry.
            Self::MemberDurableTailUnprovable { .. } => MobFailureClass::RuntimeRejected,
            Self::SessionUnavailableForResume { reason, .. } => match reason {
                SessionResumeUnavailableReason::Absent => MobFailureClass::TargetMissing,
                SessionResumeUnavailableReason::ArchivedNotRevivable => {
                    MobFailureClass::TargetArchived
                }
                SessionResumeUnavailableReason::CommittedBoundaryUnprovable => {
                    MobFailureClass::RuntimeRejected
                }
                SessionResumeUnavailableReason::AuthorityChangedDuringMaterialization => {
                    MobFailureClass::Transport
                }
                SessionResumeUnavailableReason::ContradictoryDurableAuthority => {
                    MobFailureClass::Internal
                }
            },
            // Named explicitly rather than left to the `_` arm below: the
            // route exists and is held by a live incumbent, which is the
            // "target exists but cannot accept this right now" shape, not a
            // mob-authority rejection. Classified here as a decision, because
            // `MobFailureClass` is what schedule delivery records.
            Self::MemberAlreadyExists(_)
            | Self::ParticipantNameOccupied { .. }
            | Self::MemberAdmissionBacklogFull { .. } => MobFailureClass::TargetBusy,
            Self::StorageError(_)
            | Self::SessionError(_)
            | Self::MemberProvisionFailed { .. }
            | Self::ForkMemberProvisionFailed { .. }
            | Self::CommsError(_)
            | Self::RetirementTopologyIncomplete(_)
            | Self::MemberRestoreFailed { .. }
            | Self::KickoffWaitTimedOut { .. }
            | Self::ReadyWaitTimedOut { .. }
            | Self::BridgeRequestTimedOut { .. }
            | Self::ActorCommandTimedOut { .. }
            | Self::MemberReloadRefused { .. }
            | Self::MemberReloadRepairBlocked { .. }
            | Self::MemberReloadTimedOut { .. }
            | Self::FlowTurnTimedOut
            // The input aged out before it ran: nothing was delivered, and
            // resubmitting it is correct.
            | Self::MemberInputRequeueExpired { .. } => MobFailureClass::Transport,
            Self::Internal(_)
            | Self::WorkInputCompletionUnavailable { .. }
            | Self::ExternalMemberCleanupUncertain { .. }
            // The store disagrees with itself, as
            // `ContradictoryDurableAuthority` above.
            | Self::IncarnationInconsistent { .. } => MobFailureClass::Internal,
            Self::CallbackPending { .. }
            | Self::CallbackBatchPending { .. }
            | Self::RetirementInProgress { .. }
            | Self::MemberRetirementInProgress { .. }
            | Self::MemberRetirementAdmissionPending { .. }
            | Self::LifecycleOperationProgressStalled { .. }
            | Self::LifecycleOperationAdmissionPending { .. }
            // A Shutdown in progress holds the actor; the request ran nowhere.
            | Self::ShutdownInProgress { .. }
            | Self::SupervisorProtocolUpgradeRequired { .. }
            // A degraded registration is not a temporal "busy" collision
            // (§ TargetBusy's own doc: "target exists but cannot accept
            // this right now"). It requires the explicit, non-destructive
            // `reload_member_registration` repair before ordinary work can
            // resume — the same "runtime accepted no further work until an
            // explicit action closes the gap" shape as
            // `SupervisorProtocolUpgradeRequired`, not ordinary backoff.
            | Self::MemberReloadRequired { .. }
            | Self::MemberRetirementStuck { .. }
            | Self::RetirementInterrupted { .. }
            | Self::MemberRuntimeDetached { .. }
            | Self::DirectMemberAdoptionPending { .. }
            | Self::WorkInputIdempotencyConflict { .. }
            | Self::MemberInputRefused { .. }
            // The runtime holds the input queued for the next start: it
            // completes then, never by a resubmission now.
            | Self::MemberInputRequeuedAcrossStop { .. }
            | Self::RuntimeEffectRefused { .. }
            // The host refused the restored member: only an explicit repair
            // brings it back.
            | Self::MemberRegistrationRefused { .. }
            | Self::MemberDestinationNotInstalled { .. }
            // An explicit action closes it (declare the restored mob set, or
            // destroy or repair the named held child), the same shape as
            // `MemberReloadRequired`.
            | Self::ProvenanceUnavailable { .. } => MobFailureClass::RuntimeRejected,
            _ => MobFailureClass::MobRejected,
        }
    }

    /// §17.4: the console wire projection for the four multi-host codes
    /// (ADJ-P7-4 — THE single `MobError → ErrorCode` owner; RPC, REST, MCP,
    /// and CLI all consume this, none re-derives).
    ///
    /// `None` ⇒ the surface keeps its existing rendering byte-identical
    /// (`invalid_params` / `EXIT_ERROR` / existing REST rendering).
    ///
    /// Deliberate non-mappings (pinned by test):
    /// - `CommsError(SendError::PeerOffline)` does NOT map to
    ///   `HostUnavailable` — it also names member↔member comms failures, so
    ///   laundering it into a host-liveness verdict would be a false typed
    ///   claim.
    /// - `SupervisorRotationIncomplete` keeps its bespoke
    ///   `mob_rotate_supervisor_error` renderer and richer data envelope.
    pub fn wire_detail(&self) -> Option<WireMobErrorDetail> {
        match self {
            Self::SharedRetirementFailure(error)
            | Self::SharedLifecycleFailure(error)
            | Self::MemberCustodyDeferred { cause: error, .. }
            | Self::MemberStartupRecoveryFailed { cause: error, .. } => error.wire_detail(),
            // A probe that timed out projects the timeout's detail.
            Self::HostFeaturesProbeFailed { cause, .. } => cause.wire_detail(),
            Self::DurabilityFailStop { .. } => None,
            Self::ScopeDenied(denial) => Some(WireMobErrorDetail::ScopeDenied(
                WireScopeDeniedDetail::from(denial),
            )),
            Self::StaleEventCursor {
                after_cursor,
                latest_cursor,
            } => Some(WireMobErrorDetail::StaleCursor(WireStaleCursorDetail {
                watermark: *latest_cursor,
                generation: None,
                requested: Some(*after_cursor),
            })),
            Self::StaleFenceToken {
                runtime_id,
                expected,
                actual,
            } => Some(WireMobErrorDetail::StaleFence(WireStaleFenceDetail {
                runtime_id: Some(runtime_id.to_string()),
                expected: Some(expected.get()),
                actual: Some(actual.get()),
            })),
            // The member's runtime id and fence are current; only its session
            // binding moved. Its own code, never StaleFence: the caller
            // re-captures the scope instead of re-resolving the member.
            Self::StaleDeliveryScope {
                agent_identity,
                expected_session,
                actual_session,
            } => Some(WireMobErrorDetail::StaleDeliveryScope(
                WireStaleDeliveryScopeDetail {
                    agent_identity: agent_identity.to_string(),
                    expected_session_id: expected_session.to_string(),
                    actual_session_id: actual_session.as_ref().map(ToString::to_string),
                },
            )),
            Self::StaleMemberOperatorAuthority { .. } => {
                Some(WireMobErrorDetail::StaleFence(WireStaleFenceDetail {
                    runtime_id: None,
                    expected: None,
                    actual: None,
                }))
            }
            Self::BridgeRequestTimedOut { timeout_ms, .. } => Some(
                WireMobErrorDetail::HostUnavailable(WireHostUnavailableDetail {
                    host: None,
                    timeout_ms: Some(*timeout_ms),
                }),
            ),
            Self::BridgeCommandRejected { cause, .. } => match cause {
                // Remote `StaleFence` rejections carry no fence numbers on
                // the cause — typed absence, never fabricated numbers.
                BridgeRejectionCause::StaleFence => {
                    Some(WireMobErrorDetail::StaleFence(WireStaleFenceDetail {
                        runtime_id: None,
                        expected: None,
                        actual: None,
                    }))
                }
                BridgeRejectionCause::StaleCursor {
                    watermark,
                    generation,
                } => Some(WireMobErrorDetail::StaleCursor(WireStaleCursorDetail {
                    watermark: *watermark,
                    generation: Some(*generation),
                    requested: None,
                })),
                BridgeRejectionCause::Unavailable => Some(WireMobErrorDetail::HostUnavailable(
                    WireHostUnavailableDetail {
                        host: None,
                        timeout_ms: None,
                    },
                )),
                // Already wire types on the cause — carried verbatim.
                BridgeRejectionCause::ScopeDenied {
                    required,
                    presented,
                } => Some(WireMobErrorDetail::ScopeDenied(WireScopeDeniedDetail {
                    required: *required,
                    presented: presented.clone(),
                })),
                _ => None,
            },
            _ => None,
        }
    }
}

impl crate::runtime::MobRespawnError {
    /// Delegating [`MobError::wire_detail`] (ADJ-P7-4): the `Mob(inner)`
    /// wrapper carries the inner projection; respawn-specific variants have
    /// no console wire code and render on the surface default path.
    ///
    /// Lives beside the `MobError` projection (this module owns the
    /// variant→code knowledge) rather than in `runtime/handle.rs`.
    pub fn wire_detail(&self) -> Option<WireMobErrorDetail> {
        match self {
            Self::Mob(inner) => inner.wire_detail(),
            _ => None,
        }
    }

    /// Delegate the canonical Mob-owned public status projection through
    /// wrappers that can surface retirement-in-progress after the old member
    /// has entered durable Retiring.
    pub fn wire_error_code(&self) -> Option<meerkat_contracts::ErrorCode> {
        match self {
            Self::Mob(inner) | Self::SpawnAfterRetireWithCause { cause: inner, .. } => {
                inner.wire_error_code()
            }
            _ => None,
        }
    }

    /// Structured data carried by the mob cause, including failures that
    /// happened after the previous member was retired.
    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::Mob(inner) | Self::SpawnAfterRetireWithCause { cause: inner, .. } => {
                inner.structured_data()
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{Diagnostic, DiagnosticCode, DiagnosticSeverity};

    #[test]
    fn retirement_in_progress_is_classified_through_shared_failures() {
        let member = MobError::MemberRetirementInProgress {
            member_id: AgentIdentity::from("member"),
            stage: "actor_retirement_saga".to_string(),
        };
        assert!(member.is_retirement_in_progress());
        let session = MobError::RetirementInProgress {
            session_id: meerkat_core::SessionId::new(),
            stage: "runtime_control_retire".to_string(),
        };
        assert!(session.is_retirement_in_progress());
        assert!(MobError::SharedRetirementFailure(Arc::new(member)).is_retirement_in_progress());
        assert!(
            !MobError::MemberRetirementAdmissionPending {
                member_id: AgentIdentity::from("member"),
                stage: "actor_command_admission".to_string(),
            }
            .is_retirement_in_progress(),
            "a retirement that was never admitted is not in progress"
        );
        assert!(!MobError::Internal("boom".to_string()).is_retirement_in_progress());
    }

    #[test]
    fn test_profile_not_found_display() {
        let err = MobError::ProfileNotFound(ProfileName::from("missing"));
        assert!(format!("{err}").contains("missing"));
    }

    /// The name-occupancy extractor recognises exactly the comms-owned
    /// `NameOccupied` rejection and re-carries its holder key byte-for-byte.
    /// Anything else - including the sibling `ZeroPubkey` rejection, which is
    /// the same enum and the same wrapping variant - must return `None`, so no
    /// call site can turn an unrelated comms fault into a false occupancy
    /// claim naming a key that holds nothing.
    #[test]
    fn comms_name_occupancy_holder_recognises_only_the_name_occupied_rejection() {
        let holder = meerkat_comms::PubKey([7u8; 32]);
        let occupied = meerkat_comms::CommsRuntimeError::InprocRegistrationRejected(
            meerkat_comms::RegistrationRejection::NameOccupied {
                holder_pubkey: holder,
            },
        );
        assert_eq!(
            comms_name_occupancy_holder(&occupied),
            Some(holder),
            "the holder key must be re-carried verbatim from the comms owner"
        );

        let zero_pubkey = meerkat_comms::CommsRuntimeError::InprocRegistrationRejected(
            meerkat_comms::RegistrationRejection::ZeroPubkey,
        );
        assert!(
            comms_name_occupancy_holder(&zero_pubkey).is_none(),
            "a sibling registration rejection is not a name-occupancy fact"
        );

        let unrelated = meerkat_comms::CommsRuntimeError::AlreadyStarted;
        assert!(
            comms_name_occupancy_holder(&unrelated).is_none(),
            "an unrelated comms fault is not a name-occupancy fact"
        );
    }

    /// The operator-facing remedy text is what an adopter reads at 3am. Pin
    /// that it leads with the action (retire the incumbent), names the concrete
    /// affordance that performs it, covers the shape 0.8.24's one claim rule
    /// newly refuses (the incumbent is your own predecessor under the same
    /// authority key), and explicitly disclaims the crypto reading that cost a
    /// fleet an hour in 0.8.23 - rather than only naming a public key.
    #[test]
    fn participant_name_occupied_message_states_the_remedy_not_just_the_refusal() {
        let message = participant_name_occupied_message(
            "mob/x/__mob_supervisor__",
            &meerkat_comms::PubKey([3u8; 32]),
        );
        assert!(
            message.contains("retire the incumbent"),
            "the message must name the remedy, got: {message}"
        );
        assert!(
            message.contains("MobHandle::shutdown"),
            "the message must name the affordance that performs the remedy, got: {message}"
        );
        assert!(
            message.contains("holding the key is not a claim on the route"),
            "the message must cover the same-authority case the one claim rule newly refuses, \
             got: {message}"
        );
        assert!(
            message.contains("not a key or trust failure"),
            "the message must disclaim the crypto reading, got: {message}"
        );
        assert!(
            message.contains("mob/x/__mob_supervisor__"),
            "the message must name the refused participant name, got: {message}"
        );
    }

    /// DELETE_ME A2 + B8 regression: the `Meerkat*` variant prefix and
    /// the "meerkat" literal in error messages were renamed to
    /// identity-first terminology ("mob member"). This test pins both
    /// the display-string and the variant-construction shape so the
    /// 0.6 identity-first cascade cannot regress into legacy wording.
    #[test]
    fn member_not_found_and_already_exists_use_identity_first_display() {
        let not_found = MobError::MemberNotFound(AgentIdentity::from("singer"));
        let already = MobError::MemberAlreadyExists(AgentIdentity::from("singer"));
        let not_addressable = MobError::NotExternallyAddressable(AgentIdentity::from("singer"));

        let msg_nf = format!("{not_found}");
        let msg_ae = format!("{already}");
        let msg_na = format!("{not_addressable}");

        assert_eq!(msg_nf, "mob member not found: singer");
        assert_eq!(msg_ae, "mob member already exists: singer");
        assert_eq!(msg_na, "mob member is not externally addressable: singer");

        // No legacy "meerkat" literal should appear in any of the
        // identity-first error displays.
        for msg in [&msg_nf, &msg_ae, &msg_na] {
            assert!(
                !msg.to_lowercase().contains("meerkat"),
                "identity-first mob errors must not carry legacy 'meerkat' wording: {msg}",
            );
        }
    }

    #[test]
    fn test_invalid_transition_display() {
        let err = MobError::InvalidTransition {
            from: MobState::Completed,
            to: MobState::Running,
        };
        let msg = format!("{err}");
        assert!(msg.contains("Completed"));
        assert!(msg.contains("Running"));
    }

    #[test]
    fn test_definition_error_display() {
        let err = MobError::DefinitionError(vec![
            Diagnostic {
                code: DiagnosticCode::MissingSkillRef,
                message: "skill 'foo' not found".to_string(),
                location: Some("profiles.worker.skills[0]".to_string()),
                severity: DiagnosticSeverity::Error,
            },
            Diagnostic {
                code: DiagnosticCode::EmptyProfiles,
                message: "no spawnable profiles".to_string(),
                location: Some("profiles".to_string()),
                severity: DiagnosticSeverity::Error,
            },
        ]);
        let msg = format!("{err}");
        assert!(msg.contains("missing_skill_ref"));
        assert!(msg.contains("empty_profiles"));
    }

    #[test]
    fn test_session_error_from() {
        let session_err = meerkat_core::service::SessionError::NotFound {
            id: meerkat_core::types::SessionId::new(),
        };
        let mob_err: MobError = session_err.into();
        assert!(matches!(mob_err, MobError::SessionError(_)));
    }

    #[test]
    fn test_comms_error_from() {
        let send_err = meerkat_core::comms::SendError::PeerNotFound("agent-1".to_string());
        let mob_err: MobError = send_err.into();
        assert!(matches!(mob_err, MobError::CommsError(_)));
    }

    #[test]
    fn test_storage_error() {
        let err = MobError::StorageError(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "disk full",
        )));
        assert!(format!("{err}").contains("disk full"));
    }

    #[test]
    fn member_reload_repair_blocked_projects_typed_retryable_data() {
        let session_id = meerkat_core::types::SessionId::new();
        let refused = MobError::MemberReloadRepairBlocked {
            session_id: session_id.clone(),
            reason: "commit outcome not reported".to_string(),
            evidence_digest: None,
        };
        assert_eq!(
            refused.to_string(),
            format!(
                "member session {session_id} reload is repair-blocked: commit outcome not reported"
            )
        );
        assert_eq!(
            refused.structured_data(),
            Some(serde_json::json!({
                "kind": "mob_member_reload_repair_blocked",
                "session_id": session_id.to_string(),
                "reason": "commit outcome not reported",
                "retryable": true,
                "authority_retained": true,
                "live_shell_discarded": false,
            }))
        );
        assert_eq!(
            refused.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SessionBusy)
        );
        assert_eq!(refused.failure_class(), MobFailureClass::Transport);
        assert!(!refused.is_retirement_in_progress());
        let with_digest = MobError::MemberReloadRepairBlocked {
            session_id,
            reason: "commit outcome not reported".to_string(),
            evidence_digest: Some("digest".to_string()),
        };
        assert_eq!(
            with_digest
                .structured_data()
                .and_then(|data| data.get("evidence_digest").cloned()),
            Some(serde_json::json!("digest"))
        );
    }

    /// A stuck retirement's wire data carries its cause's structured data
    /// beside the cause's text, so a client tells "redrive later" from
    /// "reload the member, then redrive" without parsing text. A cause
    /// without structured data adds no key, and the rest is unchanged.
    #[test]
    fn a_stuck_retirement_carries_its_typed_cause_data_beside_its_text() {
        let member_id = AgentIdentity::from("member");
        let typed_causes = [
            MobError::MemberReloadRepairBlocked {
                session_id: meerkat_core::types::SessionId::new(),
                reason: "commit outcome not reported".to_string(),
                evidence_digest: None,
            },
            MobError::MemberReloadRequired {
                member_id: member_id.clone(),
                reason: "durability reload required".to_string(),
            },
        ];
        for cause in typed_causes {
            let cause_text = cause.to_string();
            let cause_data = cause
                .structured_data()
                .expect("a typed cause has structured data");
            let stuck = MobError::MemberRetirementStuck {
                member_id: member_id.clone(),
                stage: "retire-ingress-detach".to_string(),
                cause: Arc::new(cause),
            };
            assert_eq!(
                stuck.structured_data(),
                Some(serde_json::json!({
                    "kind": "mob_retirement_stuck",
                    "member_id": "member",
                    "stage": "retire-ingress-detach",
                    "cause": cause_text,
                    "cause_data": cause_data,
                    "retryable": false,
                    "redrive": true,
                    "authority_retained": true,
                }))
            );
        }
        let untyped = MobError::Internal("failed to detach peer ingress".to_string());
        let cause_text = untyped.to_string();
        let stuck = MobError::MemberRetirementStuck {
            member_id,
            stage: "retire-ingress-detach".to_string(),
            cause: Arc::new(untyped),
        };
        assert_eq!(
            stuck.structured_data(),
            Some(serde_json::json!({
                "kind": "mob_retirement_stuck",
                "member_id": "member",
                "stage": "retire-ingress-detach",
                "cause": cause_text,
                "retryable": false,
                "redrive": true,
                "authority_retained": true,
            })),
            "a cause without structured data adds no cause_data key"
        );
    }

    #[test]
    fn actor_command_timeout_projects_observation_without_execution_or_retry_verdict() {
        for stage in ["actor_command_admission", "actor_command_reply"] {
            let error = MobError::ActorCommandTimedOut {
                command_kind: "SubmitWork",
                stage,
            };
            assert_eq!(
                error.structured_data(),
                Some(serde_json::json!({
                    "kind": "mob_actor_command_timed_out",
                    "command_kind": "SubmitWork",
                    "stage": stage,
                    "deadline_reached": true,
                }))
            );
        }
    }

    /// A deferred member custody restore classifies as its cause on every
    /// typed classifier, as a startup recovery failure does. Its structured
    /// data names the member, its session and the deferred restore, keeps the
    /// cause's data when it has some and lifts the cause's hold; whether a
    /// retry can pass is the cause's. It is no retirement in progress.
    #[test]
    fn a_deferred_member_custody_classifies_as_its_cause() {
        use meerkat_core::service::SessionError;
        let session_id = meerkat_core::types::SessionId::new();
        type MakeCause = fn(&meerkat_core::types::SessionId) -> SessionError;
        let causes: [MakeCause; 3] = [
            |id| SessionError::ServedElsewhere { id: id.clone() },
            |id| SessionError::HostingUnavailable { id: id.clone() },
            |id| SessionError::DurableTailHeldForRecovery { id: id.clone() },
        ];
        for make_cause in causes {
            let bare = MobError::SessionError(make_cause(&session_id));
            let deferred = MobError::MemberCustodyDeferred {
                member_id: AgentIdentity::from("w-1"),
                session_id: session_id.clone(),
                restore: MemberCustodyRestore::CleanupDelivery,
                cause: Arc::new(MobError::SessionError(make_cause(&session_id))),
            };
            assert_eq!(deferred.durable_resume_hold(), bare.durable_resume_hold());
            assert_eq!(deferred.wire_error_code(), bare.wire_error_code());
            assert_eq!(deferred.failure_class(), bare.failure_class());
            assert_eq!(
                format!("{:?}", deferred.wire_detail()),
                format!("{:?}", bare.wire_detail())
            );
            assert_eq!(
                format!("{:?}", deferred.bridge_rejection_cause()),
                format!("{:?}", bare.bridge_rejection_cause())
            );
            assert_eq!(
                deferred.is_closed_runtime_effect_refusal(),
                bare.is_closed_runtime_effect_refusal()
            );
            assert!(!deferred.is_retirement_in_progress());
            let data = deferred.structured_data().expect("structured data");
            assert_eq!(data["kind"], "mob_member_custody_deferred");
            assert_eq!(data["member_id"], "w-1");
            assert_eq!(data["session_id"], session_id.to_string());
            assert_eq!(data["restore"], "cleanup_delivery");
            assert!(
                data.get("retryable").is_none(),
                "whether a retry can pass is the cause's: {data}"
            );
            assert_eq!(data["cause"], bare.to_string());
            assert_eq!(
                data.get("cause_data"),
                bare.structured_data().as_ref(),
                "the cause's data, only when it has some"
            );
            let hold_key = SessionError::DURABLE_RESUME_HOLD_KEY;
            assert_eq!(
                data.get(hold_key).and_then(serde_json::Value::as_str),
                bare.durable_resume_hold().map(|hold| hold.as_str()),
                "the top-level hold is the cause's"
            );
            let rendered = deferred.to_string();
            assert!(
                rendered.contains("w-1")
                    && rendered.contains(&session_id.to_string())
                    && rendered.contains("cleanup delivery restore"),
                "the message names the member, its session and the restore: {rendered}"
            );
        }
        // A durable tail its recovery kept unprovable: the deferral carries
        // its hold and its failure class, and its data as `cause_data`.
        let unprovable_tail = || MobError::MemberDurableTailUnprovable {
            member_id: AgentIdentity::from("w-1"),
            session_id: session_id.clone(),
        };
        let unprovable = unprovable_tail();
        let deferred = MobError::MemberCustodyDeferred {
            member_id: AgentIdentity::from("w-1"),
            session_id: session_id.clone(),
            restore: MemberCustodyRestore::CleanupDelivery,
            cause: Arc::new(unprovable_tail()),
        };
        assert_eq!(
            deferred.durable_resume_hold(),
            Some(meerkat_core::service::DurableResumeHold::TailHeldForRecovery)
        );
        assert_eq!(deferred.failure_class(), unprovable.failure_class());
        assert_eq!(deferred.wire_error_code(), unprovable.wire_error_code());
        let data = deferred.structured_data().expect("structured data");
        assert_eq!(
            data.get("cause_data"),
            unprovable.structured_data().as_ref()
        );
        let untyped = MobError::MemberCustodyDeferred {
            member_id: AgentIdentity::from("w-1"),
            session_id,
            restore: MemberCustodyRestore::StartupBinding,
            cause: Arc::new(MobError::Internal("restore failed".to_string())),
        };
        let data = untyped.structured_data().expect("structured data");
        assert_eq!(data["restore"], "startup_binding");
        assert!(data.get("cause_data").is_none(), "{data}");
    }

    /// A member's startup recovery failure, shared with every handle,
    /// classifies exactly as its bare cause on every typed classifier, for a
    /// hosting refusal and for a cause that carries a durable resume hold.
    /// Its structured data names the member and its session, keeps the
    /// cause's data, and lifts the cause's hold to meerkat's top-level key.
    #[test]
    fn a_member_startup_recovery_failure_classifies_as_its_cause() {
        use meerkat_core::service::SessionError;
        let session_id = meerkat_core::types::SessionId::new();
        type MakeCause = fn(&meerkat_core::types::SessionId) -> SessionError;
        let causes: [MakeCause; 3] = [
            |id| SessionError::ServedElsewhere { id: id.clone() },
            |id| SessionError::HostingUnavailable { id: id.clone() },
            |id| SessionError::DurableTailHeldForRecovery { id: id.clone() },
        ];
        for make_cause in causes {
            let bare = MobError::SessionError(make_cause(&session_id));
            let failure =
                MobError::SharedLifecycleFailure(Arc::new(MobError::MemberStartupRecoveryFailed {
                    member_id: AgentIdentity::from("w-1"),
                    session_id: session_id.clone(),
                    cause: Arc::new(MobError::SessionError(make_cause(&session_id))),
                }));
            assert_eq!(failure.durable_resume_hold(), bare.durable_resume_hold());
            assert_eq!(failure.wire_error_code(), bare.wire_error_code());
            assert_eq!(failure.failure_class(), bare.failure_class());
            assert_eq!(
                format!("{:?}", failure.wire_detail()),
                format!("{:?}", bare.wire_detail())
            );
            assert_eq!(
                format!("{:?}", failure.bridge_rejection_cause()),
                format!("{:?}", bare.bridge_rejection_cause())
            );
            assert_eq!(
                failure.is_closed_runtime_effect_refusal(),
                bare.is_closed_runtime_effect_refusal()
            );
            let data = failure.structured_data().expect("structured data");
            assert_eq!(data["kind"], "mob_member_startup_recovery_failed");
            assert_eq!(data["member_id"], "w-1");
            assert_eq!(data["session_id"], session_id.to_string());
            assert_eq!(
                data["cause_data"],
                bare.structured_data().unwrap_or(serde_json::Value::Null)
            );
            let hold_key = SessionError::DURABLE_RESUME_HOLD_KEY;
            assert_eq!(
                data.get(hold_key).and_then(serde_json::Value::as_str),
                bare.durable_resume_hold().map(|hold| hold.as_str()),
                "the top-level hold is the cause's"
            );
            let rendered = failure.to_string();
            assert!(
                rendered.contains("w-1") && rendered.contains(&session_id.to_string()),
                "the message names the member and its session: {rendered}"
            );
        }
    }

    /// A durability fail-stop record wrapping `cause`, and the admission
    /// close whose answers the record gives.
    fn fail_stop_record_and_close(cause: MobError) -> (MobError, MobError) {
        let mob_id = MobId::from("fail-stopped");
        (
            MobError::DurabilityFailStop {
                mob_id: mob_id.clone(),
                cause: Arc::new(cause),
            },
            MobError::AdmissionClosed {
                mob_id,
                reason: crate::runtime::MobAdmissionCloseReason::DurabilityFailStop,
            },
        )
    }

    /// Restart, do not park: a hold in the cause belongs to a mutation the
    /// stopped actor never retries, so the record has none, and its data
    /// keeps the cause's under `cause_data` without lifting the hold.
    #[test]
    fn a_durability_fail_stop_parks_nothing_on_its_causes_hold() {
        use meerkat_core::service::SessionError;
        let session_id = meerkat_core::types::SessionId::new();
        let bare = MobError::SessionError(SessionError::DurableTailHeldForRecovery {
            id: session_id.clone(),
        });
        assert!(bare.durable_resume_hold().is_some(), "the cause holds");
        let (record, close) = fail_stop_record_and_close(MobError::SessionError(
            SessionError::DurableTailHeldForRecovery { id: session_id },
        ));
        assert_eq!(record.durable_resume_hold(), close.durable_resume_hold());
        assert!(record.durable_resume_hold().is_none());
        let data = record.structured_data().expect("structured data");
        assert_eq!(data["kind"], "mob_durability_fail_stop");
        assert_eq!(data["mob_id"], "fail-stopped");
        assert_eq!(data["reason"], "durability_fail_stop");
        assert_eq!(data["actor_stopped"], true);
        assert_eq!(
            data["cause_data"],
            bare.structured_data().unwrap_or(serde_json::Value::Null)
        );
        assert!(
            data.get(SessionError::DURABLE_RESUME_HOLD_KEY).is_none(),
            "the cause's hold is not lifted to the top-level key: {data}"
        );
    }

    #[test]
    fn a_durability_fail_stop_has_the_close_wire_code() {
        let session_id = meerkat_core::types::SessionId::new();
        let bare =
            MobError::SessionError(meerkat_core::service::SessionError::HostingUnavailable {
                id: session_id.clone(),
            });
        assert!(bare.wire_error_code().is_some(), "the cause has a code");
        let (record, close) = fail_stop_record_and_close(MobError::SessionError(
            meerkat_core::service::SessionError::HostingUnavailable { id: session_id },
        ));
        assert_eq!(record.wire_error_code(), close.wire_error_code());
    }

    #[test]
    fn a_durability_fail_stop_has_the_close_failure_class() {
        let bare = MobError::Internal("append outcome unknown".to_string());
        let (record, close) =
            fail_stop_record_and_close(MobError::Internal("append outcome unknown".to_string()));
        assert_ne!(
            bare.failure_class(),
            close.failure_class(),
            "the cause's class differs"
        );
        assert_eq!(record.failure_class(), close.failure_class());
        assert_eq!(record.failure_class(), MobFailureClass::MobRejected);
    }

    #[test]
    fn a_durability_fail_stop_has_the_close_wire_detail() {
        let stale = || MobError::StaleEventCursor {
            after_cursor: 3,
            latest_cursor: 7,
        };
        assert!(stale().wire_detail().is_some(), "the cause has a detail");
        let (record, close) = fail_stop_record_and_close(stale());
        assert_eq!(
            format!("{:?}", record.wire_detail()),
            format!("{:?}", close.wire_detail())
        );
        assert!(record.wire_detail().is_none());
    }

    #[test]
    fn a_durability_fail_stop_has_the_close_bridge_rejection_cause() {
        let rejected = || MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::StaleFence,
            reason: "stale fence".to_string(),
        };
        assert!(
            rejected().bridge_rejection_cause().is_some(),
            "the cause is a rejection"
        );
        let (record, close) = fail_stop_record_and_close(rejected());
        assert_eq!(
            record.bridge_rejection_cause().is_some(),
            close.bridge_rejection_cause().is_some()
        );
        assert!(record.bridge_rejection_cause().is_none());
    }

    #[test]
    fn a_durability_fail_stop_is_the_close_not_a_runtime_effect_refusal() {
        let refused = || MobError::RuntimeEffectRefused {
            kind: RuntimeEffectKind::RuntimeBinding,
            session_id: meerkat_core::types::SessionId::new(),
            refusal_code: "refused".to_string(),
            reason: "the runtime refused".to_string(),
        };
        assert!(
            refused().is_closed_runtime_effect_refusal(),
            "the cause is a refusal"
        );
        let (record, close) = fail_stop_record_and_close(refused());
        assert_eq!(
            record.is_closed_runtime_effect_refusal(),
            close.is_closed_runtime_effect_refusal()
        );
        assert!(!record.is_closed_runtime_effect_refusal());
    }

    /// The spawn-many failure cause, which mob/list's failure carries.
    #[test]
    fn a_durability_fail_stop_has_the_close_spawn_many_cause() {
        let bare = MobError::SupervisorEscalation("escalated".to_string());
        let (record, close) =
            fail_stop_record_and_close(MobError::SupervisorEscalation("escalated".to_string()));
        assert_ne!(
            crate::mob_error_wire_code(&bare),
            crate::mob_error_wire_code(&close),
            "the cause's own differs"
        );
        assert_eq!(
            crate::mob_error_wire_code(&record),
            crate::mob_error_wire_code(&close)
        );
    }

    #[test]
    fn a_durability_fail_stop_renders_its_mob_and_its_cause() {
        let (record, _close) =
            fail_stop_record_and_close(MobError::Internal("append outcome unknown".to_string()));
        assert_eq!(
            record.to_string(),
            "mob 'fail-stopped' fail-stopped after an uncertain durable mutation: \
             internal error: append outcome unknown"
        );
    }

    /// A fail-stop's reply can arrive wrapped in a shared lifecycle failure:
    /// the retirement topology mapper reads through it, so a topology
    /// failure keeps its one prefix and the shared cause it carries.
    #[test]
    fn the_retirement_topology_mapper_reads_through_a_shared_lifecycle_failure() {
        let inner = Arc::new(MobError::RetirementTopologyIncomplete(
            "edge a-b".to_string(),
        ));
        let mapped = MobError::SharedLifecycleFailure(Arc::clone(&inner))
            .into_retirement_topology_incomplete();
        match &mapped {
            MobError::SharedLifecycleFailure(shared) => assert!(
                Arc::ptr_eq(shared, &inner),
                "the same shared cause comes back: {mapped:?}"
            ),
            other => panic!("a shared topology failure comes back unchanged: {other:?}"),
        }
        assert_eq!(
            mapped.to_string(),
            "retirement topology cleanup incomplete: edge a-b"
        );

        let bare = MobError::RetirementTopologyIncomplete("edge c-d".to_string())
            .into_retirement_topology_incomplete();
        assert!(
            matches!(&bare, MobError::RetirementTopologyIncomplete(reason) if reason == "edge c-d"),
            "a bare topology failure comes back unchanged: {bare:?}"
        );
        for other in [
            MobError::Internal("unwire refused".to_string()),
            MobError::SharedLifecycleFailure(Arc::new(MobError::Internal(
                "unwire refused".to_string(),
            ))),
        ] {
            let rendered = other.to_string();
            let mapped = other.into_retirement_topology_incomplete();
            assert!(
                matches!(&mapped, MobError::RetirementTopologyIncomplete(reason) if *reason == rendered),
                "any other error is wrapped once with its rendering: {mapped:?}"
            );
        }
    }

    /// The spawn wiring mapper reads through a shared lifecycle failure too:
    /// a wiring failure keeps its one prefix and the shared cause it carries.
    #[test]
    fn the_wiring_mapper_reads_through_a_shared_lifecycle_failure() {
        let inner = Arc::new(MobError::WiringError("peer a-b".to_string()));
        let mapped = MobError::SharedLifecycleFailure(Arc::clone(&inner)).into_wiring_error();
        match &mapped {
            MobError::SharedLifecycleFailure(shared) => assert!(
                Arc::ptr_eq(shared, &inner),
                "the same shared cause comes back: {mapped:?}"
            ),
            other => panic!("a shared wiring failure comes back unchanged: {other:?}"),
        }
        assert_eq!(mapped.to_string(), inner.to_string());

        let bare = MobError::WiringError("peer c-d".to_string()).into_wiring_error();
        assert!(
            matches!(&bare, MobError::WiringError(reason) if reason == "peer c-d"),
            "a bare wiring failure comes back unchanged: {bare:?}"
        );
        for other in [
            MobError::Internal("trust install refused".to_string()),
            MobError::SharedLifecycleFailure(Arc::new(MobError::Internal(
                "trust install refused".to_string(),
            ))),
        ] {
            let rendered = other.to_string();
            let mapped = other.into_wiring_error();
            assert!(
                matches!(&mapped, MobError::WiringError(reason) if *reason == rendered),
                "any other error is wrapped once with its rendering: {mapped:?}"
            );
        }
    }

    /// A durable tail its recovery kept is a typed tail hold: it carries the
    /// resume hold, lifts it through the startup failure that wraps it, and
    /// is never a plain retry.
    #[test]
    fn an_unprovable_durable_tail_is_a_typed_tail_hold() {
        let session_id = meerkat_core::SessionId::new();
        let unprovable = MobError::MemberDurableTailUnprovable {
            member_id: AgentIdentity::from("w-1"),
            session_id: session_id.clone(),
        };
        assert_eq!(
            unprovable.durable_resume_hold(),
            Some(meerkat_core::service::DurableResumeHold::TailHeldForRecovery)
        );
        assert_eq!(unprovable.failure_class(), MobFailureClass::RuntimeRejected);
        let data = unprovable
            .structured_data()
            .expect("the unprovable tail carries structured data");
        assert_eq!(data["kind"], "mob_member_durable_tail_unprovable");
        assert_eq!(data["member_id"], "w-1");
        assert_eq!(data["session_id"], session_id.to_string());
        assert_eq!(data["content_retained"], true);
        let wrapped = MobError::MemberStartupRecoveryFailed {
            member_id: AgentIdentity::from("w-1"),
            session_id,
            cause: Arc::new(unprovable),
        };
        assert_eq!(
            wrapped.durable_resume_hold(),
            Some(meerkat_core::service::DurableResumeHold::TailHeldForRecovery),
            "the startup failure lifts its cause's hold"
        );
    }

    /// Only a hosting refusal or an unprovable tail lets the start defer a
    /// member's custody; every other cause, a bare revision conflict
    /// included, stays fatal.
    #[test]
    fn only_hosting_refusals_and_an_unprovable_tail_defer_custody() {
        let id = meerkat_core::SessionId::new();
        for deferrable in [
            MobError::SessionError(meerkat_core::service::SessionError::ServedElsewhere {
                id: id.clone(),
            }),
            MobError::SessionError(meerkat_core::service::SessionError::HostingUnavailable {
                id: id.clone(),
            }),
            MobError::MemberDurableTailUnprovable {
                member_id: AgentIdentity::from("w-1"),
                session_id: id.clone(),
            },
        ] {
            assert!(
                deferrable.is_deferrable_custody_cause(),
                "{deferrable} defers the member"
            );
        }
        for fatal in [
            MobError::Internal("local cleanup delivery tuple diverges".to_string()),
            MobError::SessionError(meerkat_core::service::SessionError::Store(Box::new(
                meerkat_core::SessionStoreError::TranscriptRevisionConflict {
                    id: id.clone(),
                    expected: "a".to_string(),
                    actual: "b".to_string(),
                },
            ))),
            MobError::SessionError(meerkat_core::service::SessionError::NotFound {
                id: id.clone(),
            }),
            MobError::MemberStartupRecoveryFailed {
                member_id: AgentIdentity::from("w-1"),
                session_id: id.clone(),
                cause: Arc::new(MobError::SessionError(
                    meerkat_core::service::SessionError::ServedElsewhere { id: id.clone() },
                )),
            },
        ] {
            assert!(!fatal.is_deferrable_custody_cause(), "{fatal} stays fatal");
        }
    }

    /// A durable profile's spawn on a mob without persistent sessions is
    /// refused typed: a closed refusal under the existing
    /// `CAPABILITY_UNAVAILABLE` code, naming the member and the profile,
    /// never retryable as is.
    #[test]
    fn a_durable_profile_on_a_non_persistent_mob_is_a_typed_refusal() {
        let error = MobError::NonDurableService {
            member_id: AgentIdentity::from("w-1"),
            profile: ProfileName::from("worker"),
        };
        assert_eq!(
            error.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::CapabilityUnavailable)
        );
        assert_eq!(error.failure_class(), MobFailureClass::MobRejected);
        let data = error.structured_data().expect("structured data");
        assert_eq!(data["kind"], "mob_non_durable_service");
        assert_eq!(data["member_id"], "w-1");
        assert_eq!(data["profile"], "worker");
        assert_eq!(data["retryable"], false);
        let rendered = error.to_string();
        assert!(
            rendered.contains("w-1") && rendered.contains("worker"),
            "{rendered}"
        );
    }

    #[test]
    fn test_all_variants_exist() {
        // Ensures all variants are constructible.
        let _variants: Vec<MobError> = vec![
            MobError::MemberDurableTailUnprovable {
                member_id: AgentIdentity::from("m"),
                session_id: meerkat_core::SessionId::new(),
            },
            MobError::ProfileNotFound(ProfileName::from("p")),
            MobError::MemberNotFound(AgentIdentity::from("m")),
            MobError::MemberAlreadyExists(AgentIdentity::from("m")),
            MobError::NotExternallyAddressable(AgentIdentity::from("m")),
            MobError::InvalidTransition {
                from: MobState::Creating,
                to: MobState::Running,
            },
            MobError::WiringError("w".to_string()),
            MobError::ExternalMemberCleanupUncertain {
                reason: "compensation failed".to_string(),
            },
            MobError::SupervisorRotationIncomplete {
                previous_epoch: 1,
                attempted_epoch: 2,
                attempted_public_peer_id: "peer-next".to_string(),
                rotated_peer_count: 1,
                rollback_succeeded: false,
                pending_authority_recorded: true,
                rollback_error: Some("rollback failed".to_string()),
                reason: "remote failed".to_string(),
            },
            MobError::SupervisorEpochExhausted {
                current_epoch: u64::MAX,
            },
            MobError::BridgeCommandRejected {
                cause: BridgeRejectionCause::NotBound,
                reason: "bind required".to_string(),
            },
            MobError::MemberRestoreFailed {
                member_id: AgentIdentity::from("m"),
                session_id: Some(meerkat_core::types::SessionId::new()),
                reason: "restore failed".to_string(),
                hold: None,
            },
            MobError::MemberStartupRecoveryFailed {
                member_id: AgentIdentity::from("m"),
                session_id: meerkat_core::types::SessionId::new(),
                cause: Arc::new(MobError::Internal("restore failed".to_string())),
            },
            MobError::DurabilityFailStop {
                mob_id: MobId::from("mob"),
                cause: Arc::new(MobError::Internal("append outcome unknown".to_string())),
            },
            MobError::KickoffWaitTimedOut {
                pending_member_ids: vec![AgentIdentity::from("m")],
            },
            MobError::MemberReloadRequired {
                member_id: AgentIdentity::from("m"),
                reason: "durability reload required".to_string(),
            },
            MobError::MemberAdmissionBacklogFull {
                member_id: AgentIdentity::from("m"),
                depth: 3,
            },
            MobError::ActorCommandTimedOut {
                command_kind: "SubmitWork",
                stage: "actor_command_admission",
            },
            MobError::MemberReloadRefused {
                session_id: meerkat_core::types::SessionId::new(),
                reason: "durable authority unreadable".to_string(),
            },
            MobError::MemberReloadRepairBlocked {
                session_id: meerkat_core::types::SessionId::new(),
                reason: "commit outcome not reported".to_string(),
                evidence_digest: None,
            },
            MobError::RespawnPredecessorRestrictionsUnavailable {
                agent_identity: "worker".to_string(),
                reason: "no inherited tool restriction evidence".to_string(),
            },
            MobError::MemberCustodyDeferred {
                member_id: AgentIdentity::from("m"),
                session_id: meerkat_core::types::SessionId::new(),
                restore: MemberCustodyRestore::CleanupDelivery,
                cause: Arc::new(MobError::Internal("deferred".to_string())),
            },
            MobError::NonDurableService {
                member_id: AgentIdentity::from("m"),
                profile: ProfileName::from("worker"),
            },
            MobError::MemberReloadTimedOut {
                session_id: meerkat_core::types::SessionId::new(),
                stage: "durability_reload_discard",
            },
            MobError::DefinitionError(vec![]),
            MobError::FlowNotFound(FlowId::from("f")),
            MobError::FlowFailed {
                run_id: RunId::new(),
                reason: "r".to_string(),
            },
            MobError::RunNotFound(RunId::new()),
            MobError::RunCanceled(RunId::new()),
            MobError::FlowTurnTimedOut,
            MobError::FrameDepthLimitExceeded {
                loop_id: LoopId::from("loop"),
                max_frame_depth: 1,
                current_depth: 1,
            },
            MobError::FrameAtomicPersistenceUnavailable {
                operation: FrameAtomicOperation::CasGrantNodeSlot,
            },
            MobError::SpecRevisionConflict {
                mob_id: MobId::from("mob"),
                expected: Some(2),
                actual: 3,
            },
            MobError::SchemaValidation {
                step_id: StepId::from("step"),
                message: "invalid".to_string(),
            },
            MobError::InsufficientTargets {
                step_id: StepId::from("step"),
                required: 2,
                available: 1,
            },
            MobError::TopologyViolation {
                from_role: ProfileName::from("lead"),
                to_role: ProfileName::from("worker"),
            },
            MobError::SupervisorEscalation("boom".to_string()),
            MobError::UnsupportedForMode {
                mode: crate::MobRuntimeMode::TurnDriven,
                reason: "autonomous host runtime required".to_string(),
            },
            MobError::ResetBarrier,
            MobError::StorageError(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "e",
            ))),
            MobError::SessionError(meerkat_core::service::SessionError::PersistenceDisabled),
            MobError::MemberProvisionFailed {
                cause: MemberProvisionFailureCause::CallbackTransportClosed {
                    detail: "stdio eof".to_string(),
                },
            },
            MobError::CommsError(meerkat_core::comms::SendError::PeerOffline),
            MobError::StaleFenceToken {
                runtime_id: crate::ids::AgentRuntimeId::initial(crate::ids::AgentIdentity::from(
                    "m",
                )),
                expected: FenceToken::new(1),
                actual: FenceToken::new(0),
            },
            MobError::WorkNotFound(WorkRef::new()),
            MobError::WorkCancellationUnsupported(WorkRef::new()),
            MobError::BridgeSessionNotInLiveAuthority {
                bridge_session_id: "sess-1".to_string(),
            },
            MobError::ScopeDenied(ScopeDenial {
                required: crate::control_policy::ControlScope::AdminGrants,
                presented: std::collections::BTreeSet::new(),
            }),
            MobError::FlowStepDispatchRejected {
                target: AgentIdentity::from("m"),
                kind: FlowStepDispatchRejectKind::OverlayAutonomous,
            },
            MobError::Internal("i".to_string()),
        ];
    }

    /// Phase 6: both dispatch-reject kinds render their machine-arm marker
    /// (the flow ledger matches on it) and classify as mob-rejected.
    #[test]
    fn flow_step_dispatch_rejected_display_and_classification() {
        let overlay = MobError::FlowStepDispatchRejected {
            target: AgentIdentity::from("auto-member"),
            kind: FlowStepDispatchRejectKind::OverlayAutonomous,
        };
        assert!(format!("{overlay}").contains("overlay_autonomous"));
        assert!(format!("{overlay}").contains("auto-member"));
        assert_eq!(overlay.failure_class(), MobFailureClass::MobRejected);

        let incapable = MobError::FlowStepDispatchRejected {
            target: AgentIdentity::from("eph-member"),
            kind: FlowStepDispatchRejectKind::HostIncapable,
        };
        assert!(format!("{incapable}").contains("host_incapable"));
        assert_eq!(incapable.failure_class(), MobFailureClass::MobRejected);
    }

    /// The denial Display names the required scope in its wire spelling,
    /// and the typed conversion path (`From<ScopeDenial>`) lands on the
    /// dedicated variant classified as mob-rejected (an authorization
    /// verdict, not a transport or internal fault).
    #[test]
    fn scope_denied_display_and_classification() {
        let err = MobError::from(ScopeDenial {
            required: crate::control_policy::ControlScope::AdminGrants,
            presented: std::collections::BTreeSet::from([
                crate::control_policy::ControlScope::List,
            ]),
        });
        assert_eq!(
            format!("{err}"),
            "control scope denied: requires admin_grants"
        );
        assert_eq!(err.failure_class(), MobFailureClass::MobRejected);
        assert!(!err.is_missing_target());
    }

    #[test]
    fn failure_class_partitions_missing_target_variants() {
        // The missing-target class is the authority `is_missing_target`
        // delegates to; both must agree on the same variant partition.
        let missing = [
            MobError::MobNotFound(MobId::from("mob")),
            MobError::ProfileNotFound(ProfileName::from("p")),
            MobError::MemberNotFound(AgentIdentity::from("m")),
            MobError::FlowNotFound(FlowId::from("f")),
            MobError::RunNotFound(RunId::new()),
            MobError::WorkNotFound(WorkRef::new()),
            MobError::IncarnationDestroyed {
                mob_id: MobId::from("mob"),
                incarnation: meerkat_core::service::MobIncarnationId::mint(),
            },
            MobError::IncarnationMismatch {
                mob_id: MobId::from("mob"),
                expected: Some(meerkat_core::service::MobIncarnationId::mint()),
                current: Some(meerkat_core::service::MobIncarnationId::mint()),
            },
            // A version-1 scope at an incarnation that does not honour it.
            MobError::IncarnationMismatch {
                mob_id: MobId::from("mob"),
                expected: None,
                current: Some(meerkat_core::service::MobIncarnationId::mint()),
            },
        ];
        for err in &missing {
            assert_eq!(
                err.failure_class(),
                MobFailureClass::TargetMissing,
                "{err} should classify as TargetMissing",
            );
            assert!(
                err.is_missing_target(),
                "{err} should report is_missing_target()",
            );
        }
    }

    #[test]
    fn failure_class_maps_non_missing_variants() {
        let cases = [
            (
                MobError::MemberAlreadyExists(AgentIdentity::from("m")),
                MobFailureClass::TargetBusy,
            ),
            // Classified deliberately, not by the `_ => MobRejected` wildcard:
            // the route exists and a live incumbent holds it.
            (
                MobError::ParticipantNameOccupied {
                    participant_name: "mob/x/__mob_supervisor__".to_string(),
                    holder_pubkey: meerkat_comms::PubKey([1u8; 32]),
                },
                MobFailureClass::TargetBusy,
            ),
            (
                MobError::StorageError(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "e",
                ))),
                MobFailureClass::Transport,
            ),
            (MobError::FlowTurnTimedOut, MobFailureClass::Transport),
            (
                MobError::MemberProvisionFailed {
                    cause: MemberProvisionFailureCause::CallbackTransportClosed {
                        detail: "stdio eof".to_string(),
                    },
                },
                MobFailureClass::Transport,
            ),
            (
                MobError::CallbackPending {
                    session_id: meerkat_core::types::SessionId::new(),
                    tool_use_id: "call-1".to_string(),
                    tool_name: "t".to_string(),
                    args: serde_json::Value::Null,
                    dedupe_key: None,
                },
                MobFailureClass::RuntimeRejected,
            ),
            (
                MobError::Internal("i".to_string()),
                MobFailureClass::Internal,
            ),
            (
                MobError::IncarnationInconsistent {
                    mob_id: MobId::from("mob"),
                    inconsistency: IncarnationInconsistency::RecordWithoutCreation,
                },
                MobFailureClass::Internal,
            ),
            (
                MobError::ProvenanceUnavailable {
                    mob_id: Some(MobId::from("mob")),
                },
                MobFailureClass::RuntimeRejected,
            ),
            (
                MobError::ProvenanceUnavailable { mob_id: None },
                MobFailureClass::RuntimeRejected,
            ),
            // A variant outside every explicit arm falls through to the
            // mob-rejected default.
            (MobError::ResetBarrier, MobFailureClass::MobRejected),
            (
                MobError::MobMachineRejected {
                    context: "test",
                    reason: "guard rejected".to_string(),
                },
                MobFailureClass::MobRejected,
            ),
        ];
        for (err, expected) in &cases {
            assert_eq!(err.failure_class(), *expected, "{err} misclassified");
            assert!(
                !err.is_missing_target(),
                "{err} must not report is_missing_target()",
            );
        }
    }

    #[test]
    fn external_member_cleanup_uncertainty_is_typed_and_internal() {
        let error = MobError::ExternalMemberCleanupUncertain {
            reason: "injected compensation failure".to_string(),
        };

        assert!(error.external_member_cleanup_is_uncertain());
        assert_eq!(error.failure_class(), MobFailureClass::Internal);
        assert!(format!("{error}").contains("injected compensation failure"));
        assert!(
            !MobError::WiringError("ordinary bind failure".to_string())
                .external_member_cleanup_is_uncertain()
        );
    }

    /// T-B1 (§17.4, DEC-P7B-8): `wire_detail()` maps exactly the four
    /// console codes — every `Some` arm with its detail field values
    /// (including all four `BridgeCommandRejected` causes) — and the pinned
    /// non-mappings return `None`.
    #[test]
    fn a_moved_session_scope_is_not_a_stale_fence_and_an_unscopable_member_is_unsupported() {
        let expected = meerkat_core::types::SessionId::new();
        let actual = meerkat_core::types::SessionId::new();
        let moved = MobError::StaleDeliveryScope {
            agent_identity: crate::ids::AgentIdentity::from("worker"),
            expected_session: expected.clone(),
            actual_session: Some(actual.clone()),
        };
        match moved.wire_detail() {
            Some(WireMobErrorDetail::StaleDeliveryScope(detail)) => {
                assert_eq!(detail.agent_identity, "worker");
                assert_eq!(detail.expected_session_id, expected.to_string());
                assert_eq!(detail.actual_session_id, Some(actual.to_string()));
            }
            other => panic!("StaleDeliveryScope must project its own detail, got {other:?}"),
        }
        assert_eq!(
            moved.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::StaleDeliveryScope)
        );
        let unscopable = MobError::DeliveryScopeUnavailable {
            agent_identity: crate::ids::AgentIdentity::from("peer"),
            reason: "no native session binding".to_string(),
        };
        assert!(unscopable.wire_detail().is_none());
        assert_eq!(
            unscopable.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::CapabilityUnavailable)
        );
    }

    #[test]
    fn wire_detail_maps_exactly_the_four_codes() {
        use meerkat_contracts::wire::WireControlScope;

        // ScopeDenied(denial) → ScopeDenied with the typed pair.
        let err = MobError::ScopeDenied(ScopeDenial {
            required: crate::control_policy::ControlScope::AdminHost,
            presented: std::collections::BTreeSet::from([
                crate::control_policy::ControlScope::List,
            ]),
        });
        match err.wire_detail() {
            Some(WireMobErrorDetail::ScopeDenied(detail)) => {
                assert_eq!(detail.required, WireControlScope::AdminHost);
                assert_eq!(detail.presented, vec![WireControlScope::List]);
            }
            other => panic!("ScopeDenied must project ScopeDenied detail, got {other:?}"),
        }

        // StaleEventCursor → StaleCursor { watermark: latest, requested: Some(after) }.
        let err = MobError::StaleEventCursor {
            after_cursor: 41,
            latest_cursor: 7,
        };
        match err.wire_detail() {
            Some(WireMobErrorDetail::StaleCursor(detail)) => {
                assert_eq!(detail.watermark, 7);
                assert_eq!(detail.requested, Some(41));
                assert_eq!(detail.generation, None);
            }
            other => panic!("StaleEventCursor must project StaleCursor detail, got {other:?}"),
        }

        // StaleFenceToken → StaleFence with all three fields populated.
        let runtime_id =
            crate::ids::AgentRuntimeId::initial(crate::ids::AgentIdentity::from("worker"));
        let err = MobError::StaleFenceToken {
            runtime_id: runtime_id.clone(),
            expected: FenceToken::new(2),
            actual: FenceToken::new(1),
        };
        match err.wire_detail() {
            Some(WireMobErrorDetail::StaleFence(detail)) => {
                assert_eq!(detail.runtime_id, Some(runtime_id.to_string()));
                assert_eq!(detail.expected, Some(2));
                assert_eq!(detail.actual, Some(1));
            }
            other => panic!("StaleFenceToken must project StaleFence detail, got {other:?}"),
        }

        // Actor-linearized member-operator rejection is the same typed stale
        // authority family, but the exact expected/actual residency tuple is
        // deliberately not exposed on the wire.
        let err = MobError::StaleMemberOperatorAuthority {
            member_id: crate::ids::AgentIdentity::from("worker"),
            reason: "host binding generation advanced".to_string(),
        };
        match err.wire_detail() {
            Some(WireMobErrorDetail::StaleFence(detail)) => {
                assert_eq!(detail.runtime_id, None);
                assert_eq!(detail.expected, None);
                assert_eq!(detail.actual, None);
            }
            other => {
                panic!("StaleMemberOperatorAuthority must project StaleFence detail, got {other:?}")
            }
        }

        // BridgeRequestTimedOut → HostUnavailable { timeout_ms }.
        let err = MobError::BridgeRequestTimedOut {
            request_envelope_id: "env-1".to_string(),
            timeout_ms: 15_000,
        };
        match err.wire_detail() {
            Some(WireMobErrorDetail::HostUnavailable(detail)) => {
                assert_eq!(detail.timeout_ms, Some(15_000));
                assert_eq!(detail.host, None);
            }
            other => panic!("BridgeRequestTimedOut must project HostUnavailable, got {other:?}"),
        }

        // BridgeCommandRejected cause fan-out — all four mapped causes.
        let rejected = |cause: BridgeRejectionCause| MobError::BridgeCommandRejected {
            cause,
            reason: "remote said no".to_string(),
        };
        match rejected(BridgeRejectionCause::StaleFence).wire_detail() {
            Some(WireMobErrorDetail::StaleFence(detail)) => {
                // No numbers on the remote cause — typed absence.
                assert_eq!(detail.runtime_id, None);
                assert_eq!(detail.expected, None);
                assert_eq!(detail.actual, None);
            }
            other => panic!("bridge StaleFence must project StaleFence, got {other:?}"),
        }
        match rejected(BridgeRejectionCause::StaleCursor {
            watermark: 9,
            generation: 3,
        })
        .wire_detail()
        {
            Some(WireMobErrorDetail::StaleCursor(detail)) => {
                assert_eq!(detail.watermark, 9);
                assert_eq!(detail.generation, Some(3));
                assert_eq!(detail.requested, None);
            }
            other => panic!("bridge StaleCursor must project StaleCursor, got {other:?}"),
        }
        match rejected(BridgeRejectionCause::Unavailable).wire_detail() {
            Some(WireMobErrorDetail::HostUnavailable(detail)) => {
                assert_eq!(detail.host, None);
                assert_eq!(detail.timeout_ms, None);
            }
            other => panic!("bridge Unavailable must project HostUnavailable, got {other:?}"),
        }
        match rejected(BridgeRejectionCause::ScopeDenied {
            required: WireControlScope::Live,
            presented: vec![WireControlScope::List, WireControlScope::ReadHistory],
        })
        .wire_detail()
        {
            Some(WireMobErrorDetail::ScopeDenied(detail)) => {
                assert_eq!(detail.required, WireControlScope::Live);
                assert_eq!(
                    detail.presented,
                    vec![WireControlScope::List, WireControlScope::ReadHistory]
                );
            }
            other => panic!("bridge ScopeDenied must carry the pair verbatim, got {other:?}"),
        }

        // An UNMAPPED bridge cause stays on the surface default path.
        assert!(
            rejected(BridgeRejectionCause::NotBound)
                .wire_detail()
                .is_none(),
            "unmapped bridge causes must not mint a console code"
        );

        // Pinned non-mappings: PeerOffline ≠ HostUnavailable (it also names
        // member↔member comms failures); rotation keeps its bespoke
        // renderer; missing targets and internals stay surface-default.
        let non_mapped = [
            MobError::CommsError(meerkat_core::comms::SendError::PeerOffline),
            MobError::Internal("boom".to_string()),
            MobError::MobNotFound(MobId::from("missing")),
            MobError::SupervisorRotationIncomplete {
                previous_epoch: 1,
                attempted_epoch: 2,
                attempted_public_peer_id: "peer-next".to_string(),
                rotated_peer_count: 1,
                rollback_succeeded: false,
                pending_authority_recorded: true,
                rollback_error: None,
                reason: "remote failed".to_string(),
            },
        ];
        for err in &non_mapped {
            assert!(
                err.wire_detail().is_none(),
                "{err} must NOT map to a console wire code"
            );
        }
    }

    #[test]
    fn supervisor_v5_crossing_errors_report_exact_retry_authority() {
        let upgrade = MobError::SupervisorProtocolUpgradeRequired {
            operation: "retire_member".to_string(),
            current: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V4,
            required: meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion::V5,
        };
        assert_eq!(
            upgrade.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SupervisorRotationIncomplete)
        );
        let upgrade_data = upgrade.structured_data().expect("typed upgrade data");
        assert_eq!(
            upgrade_data["kind"],
            "mob_supervisor_protocol_upgrade_required"
        );
        assert_eq!(upgrade_data["required_action"], "rotate_supervisor");
        assert_eq!(upgrade_data["authority_retained"], true);

        let adoption = MobError::DirectMemberAdoptionPending {
            current_epoch: 7,
            reason: "receiver reply lost after BindMember".to_string(),
        };
        assert_eq!(
            adoption.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SessionBusy)
        );
        let adoption_data = adoption.structured_data().expect("typed adoption data");
        assert_eq!(adoption_data["kind"], "mob_direct_member_adoption_pending");
        assert_eq!(adoption_data["current_epoch"], 7);
        assert_eq!(adoption_data["supervisor_authority_committed"], true);
        assert_eq!(adoption_data["authority_retained"], true);
        assert_eq!(adoption.failure_class(), MobFailureClass::RuntimeRejected);
    }

    /// #1105: `MemberReloadRequired` gets its own dedicated wire code and
    /// `RuntimeRejected` classification instead of the prior
    /// `SessionBusy`/`TargetBusy` busy-retry pairing — the degraded
    /// registration never clears on its own, so it must not read as an
    /// ordinary backoff-and-retry condition. A sibling admission-backlog
    /// state is checked alongside to prove the reclassification is scoped
    /// to this one variant and does not leak onto its busy neighbors.
    #[test]
    fn member_reload_required_reports_dedicated_code_and_nonretryable_classification() {
        let reload_required = MobError::MemberReloadRequired {
            member_id: AgentIdentity::from("m"),
            reason: "durable commit could not be reconciled with the live shell".to_string(),
        };

        assert_eq!(
            reload_required.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::MemberReloadRequired)
        );
        assert_ne!(
            reload_required.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SessionBusy)
        );
        assert_eq!(
            reload_required.failure_class(),
            MobFailureClass::RuntimeRejected
        );
        assert_ne!(reload_required.failure_class(), MobFailureClass::TargetBusy);

        let data = reload_required
            .structured_data()
            .expect("typed reload-required data");
        assert_eq!(data["kind"], "mob_member_reload_required");
        assert_eq!(data["retryable"], false);
        assert_eq!(data["authority_retained"], true);
        assert_eq!(data["required_action"], "reload_member_registration");

        // The neighboring busy backlog state keeps its prior mapping —
        // this reclassification is scoped to `MemberReloadRequired` alone.
        let backlog_full = MobError::MemberAdmissionBacklogFull {
            member_id: AgentIdentity::from("m"),
            depth: 3,
        };
        assert_eq!(
            backlog_full.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SessionBusy)
        );
        assert_eq!(backlog_full.failure_class(), MobFailureClass::TargetBusy);
    }

    /// T-B3 (respawn half): `MobRespawnError::wire_detail` delegates for
    /// `Mob(inner)` and returns `None` for respawn-specific variants.
    #[test]
    fn respawn_wrapper_delegates_wire_detail() {
        let delegated = crate::runtime::MobRespawnError::Mob(MobError::BridgeRequestTimedOut {
            request_envelope_id: "env-2".to_string(),
            timeout_ms: 30_000,
        });
        assert!(
            matches!(
                delegated.wire_detail(),
                Some(WireMobErrorDetail::HostUnavailable(_))
            ),
            "Mob(inner) must delegate to the inner projection"
        );

        let respawn_only = crate::runtime::MobRespawnError::NoRuntimeControl {
            identity: AgentIdentity::from("worker"),
        };
        assert!(
            respawn_only.wire_detail().is_none(),
            "respawn-specific variants have no console wire code"
        );
    }

    #[test]
    fn placed_provider_auth_rejection_projects_canonical_structured_data() {
        let failure = meerkat_core::service::SessionProviderAuthFailure {
            kind: meerkat_core::AuthErrorKind::InteractiveLoginRequired,
            provider: meerkat_core::Provider::OpenAI,
            realm_id: Some(meerkat_core::RealmId::parse("global").expect("realm")),
            binding_id: Some(meerkat_core::BindingId::parse("openai").expect("binding")),
        };
        let error = MobError::BridgeCommandRejected {
            cause: BridgeRejectionCause::MaterializeBuildRejected {
                cause: meerkat_contracts::wire::supervisor_bridge::MemberBuildRejection::from_provider_auth_failure(
                    &failure,
                ),
            },
            reason: failure.public_message(),
        };

        assert_eq!(error.structured_data(), Some(failure.structured_data()));
    }

    #[test]
    fn respawn_after_retire_preserves_structured_mob_cause() {
        let failure = meerkat_core::service::SessionProviderAuthFailure {
            kind: meerkat_core::AuthErrorKind::Expired,
            provider: meerkat_core::Provider::Anthropic,
            realm_id: None,
            binding_id: None,
        };
        let expected = failure.structured_data();
        let error = crate::runtime::MobRespawnError::SpawnAfterRetireWithCause {
            identity: AgentIdentity::from("worker"),
            cause: MobError::SessionError(meerkat_core::SessionError::provider_auth_failure(
                failure,
            )),
        };

        assert_eq!(error.structured_data(), Some(expected));
        assert!(error.wire_detail().is_none());
    }

    #[test]
    fn a_destination_the_runtime_did_not_install_projects_its_typed_data() {
        let session_id = meerkat_core::SessionId::new();
        let not_advanced = MobError::MemberDestinationNotInstalled {
            session_id: session_id.clone(),
            error: meerkat_runtime::RuntimeDriverError::ScopedContextGenerationNotAdvanced {
                bound: 7,
                requested: 7,
            },
        };
        assert_eq!(
            not_advanced.structured_data(),
            Some(serde_json::json!({
                "kind": "mob_member_destination_not_installed",
                "session_id": session_id.to_string(),
                "cause": "scoped_context_generation_not_advanced",
                "bound_generation": 7,
                "requested_generation": 7,
                "retryable": false,
            }))
        );
        let other = MobError::MemberDestinationNotInstalled {
            session_id: session_id.clone(),
            error: meerkat_runtime::RuntimeDriverError::ValidationFailed {
                reason: "a scoped destination requires the governed work host".to_string(),
            },
        };
        assert_eq!(
            other.structured_data(),
            Some(serde_json::json!({
                "kind": "mob_member_destination_not_installed",
                "session_id": session_id.to_string(),
                "cause": "validation_failed",
                "bound_generation": null,
                "requested_generation": null,
                "retryable": false,
            }))
        );
        assert_eq!(
            not_advanced.failure_class(),
            MobFailureClass::RuntimeRejected
        );
    }

    /// A keyed member send's refusals keep one wire class each, the mapping
    /// the rpc reference states: an invalid key and a conflict name no code of
    /// their own (the RPC answers invalid params), a host that cannot keep
    /// the key is a capability gap naming its cause, an unreadable status is
    /// retryable, and an inconsistent one is internal. None carries a wire
    /// detail, so the RPC renders the code and the structured data.
    #[test]
    fn keyed_member_send_refusals_keep_their_wire_codes_and_kinds() {
        use MemberSendIdempotencyRefusal as Refusal;
        use meerkat_contracts::ErrorCode;

        let cases = [
            (
                Refusal::InvalidKey {
                    detail: "the key is empty".to_string(),
                },
                None,
                serde_json::json!({"kind": "invalid_idempotency_key"}),
            ),
            (
                Refusal::Conflict {
                    delivery_id: "delivery-1".to_string(),
                    committed_at_ms: 7,
                },
                None,
                serde_json::json!({
                    "kind": "input_idempotency_conflict",
                    "delivery_id": "delivery-1",
                    "committed_at_ms": 7,
                }),
            ),
            (
                Refusal::NoContinuationService,
                Some(ErrorCode::CapabilityUnavailable),
                serde_json::json!({
                    "kind": "idempotency_unavailable",
                    "cause": "no_continuation_service",
                }),
            ),
            (
                Refusal::GovernedRuntime,
                Some(ErrorCode::CapabilityUnavailable),
                serde_json::json!({
                    "kind": "idempotency_unavailable",
                    "cause": "governed_runtime",
                }),
            ),
            (
                Refusal::Unavailable {
                    detail: "the store is busy".to_string(),
                },
                Some(ErrorCode::SessionRuntimeUnavailable),
                serde_json::json!({"kind": "continuation_status_unavailable"}),
            ),
            (
                Refusal::Inconsistent {
                    detail: "two receipts for one key".to_string(),
                },
                Some(ErrorCode::InternalError),
                serde_json::json!({"kind": "continuation_status_inconsistent"}),
            ),
        ];
        for (refusal, code, data) in cases {
            let error = MobError::MemberSendIdempotency {
                identity: AgentIdentity::from("member"),
                refusal,
            };
            assert!(error.wire_detail().is_none(), "{error}");
            assert_eq!(error.wire_error_code(), code, "{error}");
            assert_eq!(error.structured_data(), Some(data), "{error}");
        }
        assert_eq!(ErrorCode::CapabilityUnavailable.jsonrpc_code(), -32020);
        assert_eq!(ErrorCode::SessionRuntimeUnavailable.jsonrpc_code(), -32032);
        assert_eq!(ErrorCode::InternalError.jsonrpc_code(), -32603);
    }
}
