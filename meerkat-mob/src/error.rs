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
    WireStaleFenceDetail,
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
    /// (callback pending) rather than completing.
    RuntimeRejected,
    /// An internal/unexpected-state fault.
    Internal,
    /// The mob authority rejected the operation on its own terms.
    MobRejected,
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

/// Errors returned by mob operations.
#[derive(Debug, thiserror::Error)]
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

    /// A wiring operation failed.
    #[error("wiring error: {0}")]
    WiringError(String),

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
    #[error(
        "member {member_id} failed to restore {}: {reason}",
        format_member_restore_target(.session_id.as_ref())
    )]
    MemberRestoreFailed {
        member_id: AgentIdentity,
        session_id: Option<meerkat_core::types::SessionId>,
        reason: String,
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

    /// Operation is not supported for the member's runtime mode.
    #[error("unsupported for runtime mode {mode}: {reason}")]
    UnsupportedForMode {
        mode: crate::MobRuntimeMode,
        reason: String,
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

    /// The mob actor command channel closed before accepting a command.
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
    /// The source owns an active runtime turn or live actor admission. The
    /// persistent fork owner refuses to observe and copy it concurrently.
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

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("; ")
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
            crate::store::MobStoreError::FrameAtomicPersistenceUnavailable { operation } => {
                Self::FrameAtomicPersistenceUnavailable { operation }
            }
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
    /// Structured data carried by an underlying domain error.
    ///
    /// Session materialization failures use this to preserve typed
    /// provider-auth causes through MobKit and RPC without teaching the mob
    /// layer to reclassify authentication itself.
    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                error.structured_data()
            }
            Self::SessionError(error) => error.structured_data(),
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

    /// Canonical public wire status for Mob failures that carry a stable
    /// transport classification. Typed detail remains owned by
    /// [`Self::wire_detail`]; this sibling also covers retryable lifecycle
    /// states whose structured payload is not one of the four multi-host
    /// detail envelopes.
    pub fn wire_error_code(&self) -> Option<meerkat_contracts::ErrorCode> {
        self.wire_detail()
            .map(|detail| detail.code())
            .or_else(|| match self {
                Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                    error.wire_error_code()
                }
                Self::RetirementInProgress { .. }
                | Self::MemberRetirementInProgress { .. }
                | Self::MemberRetirementAdmissionPending { .. }
                | Self::LifecycleOperationPending { .. }
                | Self::LifecycleOperationProgressStalled { .. }
                | Self::LifecycleOperationAdmissionPending { .. }
                | Self::DirectMemberAdoptionPending { .. } => {
                    Some(meerkat_contracts::ErrorCode::SessionBusy)
                }
                Self::SupervisorProtocolUpgradeRequired { .. } => {
                    Some(meerkat_contracts::ErrorCode::SupervisorRotationIncomplete)
                }
                _ => None,
            })
    }

    pub fn bridge_rejection_cause(&self) -> Option<BridgeRejectionCause> {
        match self {
            Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                error.bridge_rejection_cause()
            }
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
            Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                error.is_closed_runtime_effect_refusal()
            }
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
            Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                error.failure_class()
            }
            Self::MobNotFound(_)
            | Self::ProfileNotFound(_)
            | Self::MemberNotFound(_)
            | Self::FlowNotFound(_)
            | Self::RunNotFound(_)
            | Self::WorkNotFound(_) => MobFailureClass::TargetMissing,
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
            Self::MemberAlreadyExists(_) | Self::ParticipantNameOccupied { .. } => {
                MobFailureClass::TargetBusy
            }
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
            | Self::FlowTurnTimedOut => MobFailureClass::Transport,
            Self::Internal(_) | Self::ExternalMemberCleanupUncertain { .. } => {
                MobFailureClass::Internal
            }
            Self::CallbackPending { .. }
            | Self::CallbackBatchPending { .. }
            | Self::RetirementInProgress { .. }
            | Self::MemberRetirementInProgress { .. }
            | Self::MemberRetirementAdmissionPending { .. }
            | Self::LifecycleOperationProgressStalled { .. }
            | Self::LifecycleOperationAdmissionPending { .. }
            | Self::SupervisorProtocolUpgradeRequired { .. }
            | Self::DirectMemberAdoptionPending { .. }
            | Self::RuntimeEffectRefused { .. } => MobFailureClass::RuntimeRejected,
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
            Self::SharedRetirementFailure(error) | Self::SharedLifecycleFailure(error) => {
                error.wire_detail()
            }
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
    fn test_all_variants_exist() {
        // Ensures all variants are constructible.
        let _variants: Vec<MobError> = vec![
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
            },
            MobError::KickoffWaitTimedOut {
                pending_member_ids: vec![AgentIdentity::from("m")],
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
                },
                MobFailureClass::RuntimeRejected,
            ),
            (
                MobError::Internal("i".to_string()),
                MobFailureClass::Internal,
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
}
