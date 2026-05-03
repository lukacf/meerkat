//! MeerkatMachine DSL definition with real bridging types.
use super::OptionValueExt;
use meerkat_machine_dsl::machine;

// ---------------------------------------------------------------------------
// Bridging types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AgentRuntimeId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FenceToken(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Generation(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RunId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct InputId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorkId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OperationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WaitRequestId(pub String);

/// Typed async-operation kind. Closed mirror of
/// [`meerkat_core::ops_lifecycle::OperationKind`] — replaces the former
/// newtype wrapper around an opaque JSON-encoded string. The DSL writes this
/// variant directly on `RegisterOp` so guards on `PeerReadyOp`
/// (`kind_is_mob_member_child`) can reason about the closed set without
/// string parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OperationKind {
    #[default]
    MobMemberChild,
    BackgroundToolOp,
}

/// Typed mirror of [`meerkat_core::Provider`] for use inside DSL bridging
/// types. Closed 5-variant enum; the seam carries the discriminant directly
/// rather than a JSON-encoded string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Provider {
    #[default]
    Anthropic,
    OpenAI,
    Gemini,
    SelfHosted,
    Other,
}

/// Typed mirror of [`meerkat_core::ConnectionRef`] — structural string
/// projection carrying the flat forms of `realm` / `binding` / `profile`
/// with bidirectional `From`.
///
/// The DSL layer keeps string fields because this mirror is the
/// DSL-layer identity carrier (used inside runtime-owned guards /
/// transitions where slug validation has already happened at the
/// boundary). Domain-side `ConnectionRef` carries the typed atoms
/// (`RealmId` / `BindingId` / `ProfileId`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ConnectionRef {
    pub realm_id: String,
    pub binding_id: String,
    pub profile_id: Option<String>,
}

/// Fallible conversion — DSL-layer flat strings may be slug-invalid
/// (the DSL mirror intentionally accepts opaque strings to survive
/// deserialization drift across schema versions), so lifting back to
/// Typed mirror of [`meerkat_core::SessionLlmIdentity`] — structural field
/// projection with typed `Provider` and `ConnectionRef` mirrors. The
/// `provider_params` payload is a legitimately open-set `serde_json::Value`
/// at the persistence boundary (arbitrary provider-specific options), so it
/// rides on a stable JSON-serialization field inside the DSL — never parsed
/// back as a discriminant inside any guard or transition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionLlmIdentity {
    pub model: String,
    pub provider: Provider,
    pub self_hosted_server_id: Option<String>,
    /// Stable JSON serialization of the open-set `provider_params` payload.
    /// Carried as an opaque identity token; DSL guards never inspect its
    /// content. Boundary-legitimate per the dogma round-4 brief's
    /// "variable JSON payload" carve-out applied at field granularity.
    pub provider_params_repr: Option<String>,
    pub connection_ref: Option<ConnectionRef>,
}

/// Typed mirror of [`meerkat_core::SessionToolVisibilityState`] —
/// structural projection using typed `ToolFilter` / `ToolVisibilityWitness`
/// mirrors plus ordered name sets for deterministic Ord/Hash.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionToolVisibilityState {
    pub capability_base_filter: ToolFilter,
    pub inherited_base_filter: ToolFilter,
    pub active_filter: ToolFilter,
    pub staged_filter: ToolFilter,
    pub active_requested_deferred_names: std::collections::BTreeSet<String>,
    pub staged_requested_deferred_names: std::collections::BTreeSet<String>,
    pub active_revision: u64,
    pub staged_revision: u64,
    pub requested_witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    pub filter_witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
}

/// Typed mirror of
/// [`crate::meerkat_machine_types::SessionLlmCapabilitySurface`] — structural
/// projection of the boolean capability matrix plus optional call timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionLlmCapabilitySurface {
    pub supports_temperature: bool,
    pub supports_thinking: bool,
    pub supports_reasoning: bool,
    pub inline_video: bool,
    pub vision: bool,
    pub image_tool_results: bool,
    pub supports_web_search: bool,
    pub realtime: bool,
    pub call_timeout_secs: Option<u64>,
}

/// Typed capability-surface resolution status. Closed mirror of
/// [`crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus`] —
/// replaces the former JSON-stringified wrapper the DSL used to carry the
/// two-state discriminant across the seam.
///
/// The DSL stores the variant directly on `ReconfigureSessionLlmIdentity`
/// flow state; the shell maps to/from the domain enum via the `From` impls
/// below — no `serde_json::to_string`, no string compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionLlmCapabilitySurfaceStatus {
    Resolved,
    #[default]
    Unresolved,
}

/// Typed mirror of
/// [`crate::meerkat_machine_types::SessionToolVisibilityDelta`] — structural
/// projection using typed `ToolFilter` mirrors plus the two boolean change
/// flags. Replaces the former `format!("{id:?}")` Debug-stringified wrapper.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionToolVisibilityDelta {
    pub previous_capability_base_filter: ToolFilter,
    pub current_capability_base_filter: ToolFilter,
    pub committed_visible_set_changed: bool,
    pub revision_bumped: bool,
}

/// Typed mirror of [`meerkat_core::ToolFilter`] — closed 3-variant
/// discriminant with a `BTreeSet<String>` name payload for
/// `Allow`/`Deny` so the value is `Ord + Hash` and deterministic across
/// iteration, matching the R3 `InputAbandonReason::MaxAttemptsExhausted {
/// attempts }` pattern of carrying the discriminant's companion data in a
/// field with stable ordering.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum ToolFilter {
    #[default]
    All,
    Allow(std::collections::BTreeSet<String>),
    Deny(std::collections::BTreeSet<String>),
}

/// Typed mirror of [`meerkat_core::types::ToolSourceKind`] — closed
/// 10-variant discriminant for tool provenance classification.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum ToolSourceKind {
    #[default]
    Builtin,
    Shell,
    Comms,
    Memory,
    Schedule,
    Mob,
    MobTasks,
    Callback,
    Mcp,
    RustBundle,
}

/// Typed mirror of [`meerkat_core::types::ToolProvenance`] — structural
/// projection carried inside [`ToolVisibilityWitness`], using the typed
/// `ToolSourceKind` discriminant mirror.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ToolProvenance {
    pub kind: ToolSourceKind,
    pub source_id: String,
}

/// Typed mirror of [`meerkat_core::ToolVisibilityWitness`] — structural
/// projection of the two optional witness fields.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ToolVisibilityWitness {
    pub stable_owner_key: Option<String>,
    pub last_seen_provenance: Option<ToolProvenance>,
}

impl ToolVisibilityWitness {
    fn len(&self) -> u64 {
        u64::from(self.last_seen_provenance.is_some())
    }
}

/// Per-session realtime binding-state lifecycle.
///
/// Unit variants only — carried inside `MeerkatMachine` state as a closed
/// set of phases. The default (`Unbound`) is paired with
/// `realtime_binding_authority_epoch == None` by the
/// `realtime_binding_epoch_consistency` invariant.
///
/// Default serde tagging reuses the variant names as string values
/// (`"Unbound"`, `"BindingNotReady"`, `"BindingReady"`, `"ReplacementPending"`)
/// to preserve wire-format compatibility with earlier stringly-typed clients.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RealtimeBindingState {
    #[default]
    Unbound,
    BindingNotReady,
    BindingReady,
    ReplacementPending,
}

/// Per-session realtime reconnect retry lifecycle.
///
/// The realtime websocket shell supplies time observations and jittered retry
/// deadlines, but the canonical cycle phase, attempt count, exhaustion, and
/// public status projection live in the MeerkatMachine state. This replaces
/// the previous websocket-local retry progress projection path.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RealtimeReconnectCycleState {
    #[default]
    Idle,
    Reconnecting,
    Exhausted,
}

/// Product-turn lifecycle phase for a provider-managed realtime session
/// (U9 / dogma #4).
///
/// The realtime-WS shell previously tracked turn lifecycle as three shell
/// locals (`product_turn_in_flight`, `product_turn_committed`,
/// `product_output_started`). This enum collapses the product of those
/// three orthogonal milestones into a closed set of phases the DSL owns:
///
/// - `Idle`: between turns — no input accepted yet.
/// - `AwaitingProgress`: input accepted; no commit, no output observed.
/// - `Committed`: `TurnCommitted` arrived but no output delta yet.
/// - `OutputStarted`: output delta / tool call arrived but no commit yet.
/// - `Preemptible`: both `TurnCommitted` and output have landed — the
///   only state in which an input chunk should preempt the current
///   provider-managed turn (the "committed turn has visible assistant-side
///   progress" rule documented on `should_preempt_on_input`).
///
/// Transitions are idempotent via guard rejection: the runtime handle
/// reports guard-rejected transitions as `Ok(false)` so the shell can
/// fire unconditionally on every lifecycle event without tracking its
/// own phase.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RealtimeProductTurnPhase {
    #[default]
    Idle,
    AwaitingProgress,
    Committed,
    OutputStarted,
    Preemptible,
}

/// Projection-freshness discriminant for the realtime provider session
/// (dogma round 2, U-C / dogma #1, #3, #13, #20).
///
/// Replaces the shell-local `ProjectionFreshness` enum previously owned by
/// `meerkat-rpc::realtime_ws`. Freshness truth is now canonical DSL state
/// owned by the session's MeerkatMachine; the realtime-WS shell reads it via
/// the [`RealtimeProductTurnHandle`] and fires typed inputs for each
/// observer tick, turn terminal, and refresh-drain.
///
/// The `baseline_ms` companion field
/// ([`MeerkatMachineState::realtime_projection_frontier_ms`]) pairs with this
/// discriminant: it holds the `baseline_ms` while `Clean`, and the
/// `new_at_ms` of the pending advance while `StaleDeferred` / `StaleImmediate`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RealtimeProjectionFreshness {
    /// Provider projection matches canonical session state as of
    /// `realtime_projection_frontier_ms`. No refresh owed.
    #[default]
    Clean,
    /// Canonical state advanced while the provider turn was live; refresh
    /// blocked until the turn terminates so barge-in continuity isn't broken.
    StaleDeferred,
    /// Refresh owed at the next drain site (idle input-chunk arrival or
    /// turn-end).
    StaleImmediate,
}

/// Typed classification of a clean provider-session close for the realtime
/// socket (dogma round 2, U-C / dogma #1, #3, #18, #20).
///
/// Replaces the shell-local boolean pair (`client_has_submitted_input`,
/// `last_turn_terminally_completed`) previously owned by the realtime-WS
/// dispatch loop. The DSL owns the classification; the shell reads
/// [`RealtimeProductTurnHandle::reconnect_policy_on_clean_close`] at the
/// clean-close branch point and dispatches on the typed value.
///
/// Semantics: a `CleanExit` means the session has no in-flight client work
/// that would need to be recovered via reattach (either the client never
/// submitted anything, or the last turn reached a terminal completion).
/// `ReattachAndRecover` means the client issued work that has not yet
/// reached a terminal completion, so a clean close is treated as a
/// mid-work disconnect and the channel proactively re-opens.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum RealtimeReconnectPolicy {
    /// A clean close has nothing to recover — either the client never
    /// submitted input on this session, or the last observed turn reached
    /// a terminal completion.
    #[default]
    CleanExit,
    /// The client issued work that has not yet reached a terminal turn
    /// completion; a clean close is a mid-work disconnect and the channel
    /// should proactively reattach.
    ReattachAndRecover,
}

/// Bridging type for an MCP server identifier, matching the catalog type.
/// Used as the key in `mcp_server_states` and carried on MCP lifecycle
/// inputs and effects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct McpServerId(pub String);

/// Bridging wrapper mapping [`meerkat_core::PeerCorrelationId`] into the DSL
/// macro's type system. Keyed map values for `pending_peer_requests` and
/// `inbound_peer_requests`; carried on every W1-A peer-lifecycle input and
/// effect.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerCorrelationId(pub String);

/// Typed outbound peer-request state, mirroring
/// [`meerkat_core::OutboundPeerRequestState`]. Unit variants only; failure
/// reason travels on the `PeerResponseTerminalArrived` input's companion
/// fields, not in the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OutboundPeerRequestState {
    #[default]
    Sent,
    AcceptedProgress,
    Completed,
    Failed,
    TimedOut,
}

/// Typed inbound peer-request state, mirroring
/// [`meerkat_core::InboundPeerRequestState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InboundPeerRequestState {
    #[default]
    Received,
    Replied,
}

/// Typed terminal disposition carried on `PeerResponseTerminalArrived`.
/// Mirror of [`meerkat_core::handles::PeerTerminalDisposition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerTerminalDisposition {
    #[default]
    Completed,
    Failed,
}

/// Typed lifecycle state of an interaction stream reservation (U6 / dogma #5).
///
/// Owns whether a reserved subscriber/stream channel is still claimable
/// (`Reserved`), live with an attached consumer (`Attached`), or terminal
/// (`Completed` after a terminal event won, `Expired` after the TTL elapsed
/// without an attach, `ClosedEarly` after the consumer dropped the stream
/// before terminal). Mirror of [`meerkat_core::InteractionStreamState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InteractionStreamState {
    #[default]
    Reserved,
    Attached,
    Completed,
    Expired,
    ClosedEarly,
}

/// Per-server MCP connection lifecycle state. Matches the catalog copy;
/// unit variants only so the DSL can reason about state via map inserts.
/// Failure detail travels on the `McpServerFailed` input and
/// `McpServerStateChanged` effect's companion fields, not on the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum McpServerState {
    #[default]
    PendingConnect,
    Connected,
    Failed,
    Disconnected,
}

/// Stable identity of a comms runtime instance (W2-G / issue #264).
///
/// The runtime derives this string from the `Arc<dyn CommsRuntime>` pointer
/// address via `CommsRuntimeId::from_runtime()`. The DSL treats it as an
/// opaque newtype; two distinct `Arc`s produce distinct ids so the owner
/// invariant can catch silent transport swaps.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CommsRuntimeId(pub String);

/// Mob instance identifier for peer-ingress ownership (W2-G / issue #264).
///
/// Bridging newtype mirroring `meerkat_mob::ids::MobId`. The DSL layer keeps
/// this opaque because `meerkat-runtime` does not depend on `meerkat-mob`;
/// the shell stringifies the real `MobId` before firing `AttachMobIngress`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MobId(pub String);

/// Parsed transport envelope class for peer ingress.
///
/// This is the mechanical shape comms may derive from a wire envelope before
/// semantic admission. The DSL consumes it to own the peer-input class,
/// auth-exemption, lifecycle, silent-routing, and response-terminal facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressEnvelopeClass {
    #[default]
    Message,
    Request,
    Lifecycle,
    Response,
    Ack,
}

/// DSL-owned admitted ingress kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressAdmittedKind {
    #[default]
    Message,
    Request,
    Response,
    Ack,
    PlainEvent,
}

/// DSL-owned peer input class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressInputClass {
    #[default]
    ActionableMessage,
    ActionableRequest,
    ResponseProgress,
    ResponseTerminal,
    PeerLifecycleAdded,
    PeerLifecycleRetired,
    PeerLifecycleUnwired,
    SilentRequest,
    Ack,
    PlainEvent,
}

/// DSL-owned peer lifecycle classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressLifecycleClass {
    #[default]
    PeerAdded,
    PeerRetired,
    PeerUnwired,
}

/// DSL-owned peer ingress auth classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressAuthClass {
    #[default]
    Required,
    SupervisorBridgeExempt,
}

/// Parsed response status for peer ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressResponseStatus {
    #[default]
    Accepted,
    Completed,
    Failed,
}

/// DSL-owned response progress/terminal classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressResponseTerminality {
    #[default]
    Progress,
    TerminalCompleted,
    TerminalFailed,
}

/// Peer-ingress transport capability ownership kind (W2-G / issue #264).
///
/// Paired with `peer_ingress_comms_runtime_id` and `peer_ingress_mob_id` in
/// DSL state; `peer_ingress_owner_consistency` enforces pairing. Silent
/// downgrade `MobOwned` → `SessionOwned` is structurally impossible:
/// `AttachSessionIngress` requires `Unattached`; `AttachMobIngress` permits
/// `Unattached` or `SessionOwned` but never `MobOwned` → `SessionOwned`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum PeerIngressOwnerKind {
    #[default]
    Unattached,
    SessionOwned,
    MobOwned,
}

/// Supervisor-bridge authorization kind (Wave 3 D Row 21).
///
/// Paired with `supervisor_bound_{name, peer_id, address, epoch}` in DSL
/// state; `supervisor_binding_consistency` enforces pairing. Rotation is
/// structural: `BindSupervisor` requires `Unbound`; `AuthorizeSupervisor`
/// requires `Bound`; `RevokeSupervisor` requires `Bound` and returns to
/// `Unbound`. Before Wave 3 D this fact lived as an `Option<AuthorizedSupervisorState>`
/// on the comms drain task's stack — the identity and epoch of the
/// authorized supervisor were helper-local while the corresponding trust
/// edge was router-owned. Moving the authorization discriminant + epoch
/// into DSL state collapses that split ownership.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum SupervisorBindingKind {
    #[default]
    Unbound,
    Bound,
}

/// Typed turn-execution phase, mirrored 1:1 by the closed set of literals the
/// DSL transitions assign to `turn_phase`. Replaces the prior stringly-typed
/// encoding so the ephemeral driver and runtime handles consume an exhaustive
/// enum instead of parsing folklore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnPhase {
    #[default]
    Ready,
    ApplyingPrimitive,
    CallingLlm,
    WaitingForOps,
    DrainingBoundary,
    Extracting,
    ErrorRecovery,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

/// Typed registration substate. Closed set of literals previously assigned to
/// `registration_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RegistrationPhase {
    #[default]
    Queuing,
    Active,
}

/// Typed comms drain substate. Mirrors the closed set of literals the DSL
/// transitions assign to `drain_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DrainPhase {
    #[default]
    Inactive,
    Running,
    Stopped,
    ExitedRespawnable,
}

/// Typed comms drain mode. Mirrors `crate::meerkat_machine::CommsDrainMode`
/// (which is the shell-side enum) so the DSL can hold a closed set of typed
/// variants instead of a `Debug`-formatted string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DrainMode {
    #[default]
    Timed,
    AttachedSession,
    PersistentHost,
}

/// Typed external-tool surface global phase. Closed set of literals previously
/// assigned to `surface_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SurfacePhase {
    #[default]
    Operating,
    Shutdown,
}

/// Typed live-topology reconfigure phase. Closed set of literals previously
/// assigned to `live_topology_phase`. The catalog DSL holds a parallel copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveTopologyPhase {
    #[default]
    Idle,
    Reconfiguring,
    Detached,
    HostIdentityApplied,
    HostVisibilityApplied,
}

/// Typed input-lifecycle phase, mirroring the closed set of literals the DSL
/// transitions assign to `input_phases`. The shell projects from this onto the
/// richer `crate::input_state::InputLifecycleState` (which keeps an `Accepted`
/// pre-DSL-admission variant the DSL itself never writes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputPhase {
    #[default]
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

/// Typed input terminal kind, mirroring the closed set of literals the DSL
/// transitions assign to `input_terminal_kind`. The companion fields
/// (`input_superseded_by`, `input_aggregate_id`, `input_abandon_reason`,
/// `input_abandon_attempt_count`) carry payload metadata for variants that
/// need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputTerminalKind {
    #[default]
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

/// Typed pending external-surface op. Closed set of literals previously
/// assigned to `surface_pending_op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SurfacePendingOp {
    #[default]
    None,
    Add,
    Reload,
}

/// Typed staged external-surface op. Closed set of literals previously
/// assigned to `surface_staged_op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SurfaceStagedOp {
    #[default]
    None,
    Add,
    Remove,
    Reload,
}

/// Typed turn primitive kind. Closed mirror of
/// [`meerkat_core::turn_execution_authority::TurnPrimitiveKind`] — replaces the
/// former literal-string `primitive_kind` field and `StartConversationRun`
/// input field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnPrimitiveKind {
    #[default]
    None,
    ConversationTurn,
    ImmediateAppend,
    ImmediateContextAppend,
}

/// Typed turn primitive content shape. Closed mirror of
/// [`meerkat_core::turn_execution_authority::ContentShape`] — replaces the
/// former literal-string `admitted_content_shape` state and input fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ContentShape {
    #[default]
    Conversation,
    ConversationAndContext,
    Context,
    Empty,
    ImmediateAppend,
    ImmediateContext,
}

/// Typed turn terminal outcome. Closed mirror of
/// [`meerkat_core::turn_execution_authority::TurnTerminalOutcome`] — replaces
/// the former literal-string `terminal_outcome` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnTerminalOutcome {
    #[default]
    None,
    Completed,
    Failed,
    Cancelled,
    BudgetExhausted,
    TimeBudgetExceeded,
    StructuredOutputValidationFailed,
}

/// Typed turn terminal cause. Closed mirror of
/// [`meerkat_core::turn_execution_authority::TurnTerminalCauseKind`] carried by
/// MeerkatMachine terminal failure inputs/effects so display messages cannot
/// classify terminal failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnTerminalCauseKind {
    #[default]
    Unknown,
    HookDenied,
    HookFailure,
    LlmFailure,
    ToolFailure,
    StructuredOutputValidationFailed,
    BudgetExhausted,
    TimeBudgetExceeded,
    RetryExhausted,
    TurnLimitReached,
    RuntimeApplyFailure,
    FatalFailure,
}

/// Typed classifier for failures surfaced by the runtime apply loop when a
/// `CoreExecutor::apply` call fails and terminalizes the runtime turn.
/// The companion `last_runtime_apply_failure_message` state field carries the
/// human-readable projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeApplyFailureCause {
    #[default]
    Unknown,
    PrimitiveRejected,
    RuntimeContextApply,
    RuntimeTurn,
    HookDenied,
    HookRuntimeFailure,
    ExecutorStopped,
    ExecutorControlFailed,
    ExecutorInternal,
}

/// Typed pre-run phase marker. Closed set: `idle`, `attached`, `retired`.
/// Replaces the former literal-string `pre_run_phase` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PreRunPhase {
    #[default]
    Idle,
    Attached,
    Retired,
}

/// Typed runtime notice classifier for the `RuntimeNotice` effect. Closed set
/// of per-transition runtime lifecycle markers (drain exited, runtime reset,
/// executor stopped/exited, runtime recovered) emitted by the runtime-control
/// plane. Replaces the former literal-string `kind` field on `RuntimeNotice`
/// so the shell dispatcher matches exhaustively on a typed discriminant
/// instead of comparing string literals. `detail` stays `String` — it's a
/// free-form diagnostic message that accompanies the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeNoticeKind {
    #[default]
    Drain,
    Reset,
    Stop,
    Exit,
    Recover,
}

/// Closed classifier for runtime-loop executor effects emitted as neutral DSL
/// facts before `meerkat-runtime` converts them to sealed executable effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeEffectKind {
    #[default]
    CancelAfterBoundary,
    StopRuntimeExecutor,
}

/// Typed reason classifier for the `TurnRunCancelled` effect. Closed set of
/// cancellation-observation origins emitted when a turn's cancellation
/// request lands at an observable boundary. Replaces the former literal-
/// string `reason` field on `TurnRunCancelled`. Only one origin is emitted
/// today (`Observed`, fired by the `CancellationObserved` transition), but
/// this remains a closed classifier not a free-form message — future
/// cancellation origins extend the enum rather than reintroducing strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnCancellationReason {
    #[default]
    Observed,
}

/// Typed recoverable LLM retry failure classifier. Closed mirror of
/// [`meerkat_core::retry::LlmRetryFailureKind`] so retry authority records the
/// retry cause as data, not as a parsed diagnostic string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LlmRetryFailureKind {
    #[default]
    RateLimited,
    NetworkTimeout,
    CallTimeout,
    RetryableProviderError,
}

/// Typed admission-signal classifier for the `PostAdmissionSignal` effect.
/// Closed set of post-admission wake/interrupt intents emitted by the
/// ingress authority so the shell dispatcher matches exhaustively on a
/// typed discriminant instead of comparing string literals. Mirrors the
/// shell-side `driver::ephemeral::PostAdmissionSignal` strength ordering
/// (WakeLoop < InterruptYielding < RequestImmediateProcessing); the
/// shell enum additionally carries a `None` bottom that the DSL never
/// emits, so only the three emitted variants appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PostAdmissionSignalKind {
    #[default]
    WakeLoop,
    InterruptYielding,
    RequestImmediateProcessing,
}

/// Typed base lifecycle state for an external tool surface. Closed mirror of
/// [`meerkat_core::tool_scope::ExternalToolSurfaceBaseState`] — replaces the
/// former literal-string values in `surface_base_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ExternalToolSurfaceBaseState {
    #[default]
    Absent,
    Active,
    Removing,
    Removed,
}

/// Typed last-delta operation for an external tool surface. Closed mirror of
/// [`meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation`] — replaces
/// the former literal-string values in `surface_last_delta_operation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ExternalToolSurfaceDeltaOperation {
    #[default]
    None,
    Add,
    Remove,
    Reload,
}

/// Typed last-delta phase for an external tool surface. Closed mirror of
/// [`meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase`] — replaces the
/// former literal-string values in `surface_last_delta_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ExternalToolSurfaceDeltaPhase {
    #[default]
    None,
    Pending,
    Applied,
    Draining,
    Failed,
    Forced,
}

/// Typed failure cause for an external tool surface. Closed mirror of
/// [`meerkat_core::tool_scope::ExternalToolSurfaceFailureCause`] — replaces
/// the former literal-string pending-failure and call-rejection reason fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ExternalToolSurfaceFailureCause {
    #[default]
    PendingFailed,
    SurfaceDraining,
    SurfaceUnavailable,
}

/// Typed drain-exit reason. Closed mirror of
/// [`meerkat_core::handles::DrainExitReason`] — replaces the former
/// literal-string `reason` field on `NotifyDrainExited`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DrainExitReason {
    #[default]
    IdleTimeout,
    Dismissed,
    Failed,
    Aborted,
    SessionShutdown,
}

/// Typed work-lane origin for [`MeerkatMachineInput::Ingest`]. Closed set of
/// the work-lane labels the DSL observes on the admission seam — replaces
/// the former literal-string `origin` field. Structurally mirrors the
/// `MobMachine.RequestRuntimeIngress.origin` seam so the cross-machine
/// composition binds on a single typed enum instead of parallel
/// string-typed slots. Transport sources ([`meerkat_core::comms::InputSource`])
/// arriving from the shell side collapse to `External`; the
/// runtime-control-plane `Ingest` dispatch uses the dedicated `Ingest`
/// variant; mob-bridged ingress carries `External`/`Internal` matching
/// `meerkat-mob::ids::WorkOrigin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WorkOrigin {
    #[default]
    External,
    Internal,
    /// Canonical admission entrypoint fired by the runtime control plane
    /// with no surface-level transport or work-lane label.
    Ingest,
}

/// Typed async-operation lifecycle status. Closed mirror of
/// [`meerkat_core::ops_lifecycle::OperationStatus`] — replaces the former
/// literal-string values in the DSL's `op_statuses` map.
///
/// The DSL writes these variants directly on each ops lifecycle transition
/// (`RegisterOp`, `StartOp`, `CompleteOp`, `FailOp`, `CancelOp`, `AbortOp`,
/// `RetireRequestedOp`, `RetireCompletedOp`, `TerminateOp`). The shell's
/// `ShellState::status()` reads the typed value directly and maps to the
/// domain enum via the `From` impl below — no string compares, no string
/// parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OperationStatus {
    #[default]
    Absent,
    Provisioning,
    Running,
    Retiring,
    Completed,
    Failed,
    Aborted,
    Cancelled,
    Retired,
    Terminated,
}

/// Typed discriminant mirror of
/// [`meerkat_core::ops_lifecycle::OperationTerminalOutcome`] — replaces the
/// former opaque JSON string carried in the DSL's `op_terminal_outcomes`
/// map. Unit variants only; payload data (completion result, failure error,
/// cancellation reason, terminated reason) rides on the companion
/// `op_terminal_payload: Map<String, String>` field of the DSL state as
/// JSON keyed to the same operation id, and is reconstructed in the shell
/// by pairing the typed discriminant with the companion entry.
///
/// The DSL writes these variants directly on each terminal transition
/// (`CompleteOp`, `FailOp`, `CancelOp`, `AbortOp`, `RetireCompletedOp`,
/// `TerminateOp`); the shell reads them through the typed map and rebuilds
/// the domain enum in `ShellState::terminal_outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OperationTerminalOutcomeKind {
    #[default]
    Completed,
    Failed,
    Aborted,
    Cancelled,
    Retired,
    Terminated,
}

/// Typed input-abandonment reason. Closed mirror of the discriminant set of
/// [`crate::input_state::InputAbandonReason`] — replaces the former
/// `format!("{reason:?}")` Debug round-trip in the DSL's
/// `input_abandon_reason` map.
///
/// The `MaxAttemptsExhausted` variant's `attempts` payload rides on the
/// companion `input_abandon_attempt_count: Map<String, u64>` field of the
/// DSL state; this enum only carries the discriminant. The domain
/// `InputAbandonReason::MaxAttemptsExhausted { attempts }` is reconstructed
/// in the driver by pairing the typed discriminant with that companion map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputAbandonReason {
    #[default]
    Retired,
    Reset,
    Stopped,
    Destroyed,
    Cancelled,
    MaxAttemptsExhausted,
}

/// Typed work-lane assignment for admitted inputs. Replaces the former
/// parallel `queue_lane` / `steer_lane` sets with a single map
/// (`input_lane: Map<String, Enum<InputLane>>`) so mutual exclusion is
/// structural — an admitted input is in exactly one lane by construction.
///
/// DSL-side mirror of the shell's `meerkat_core::types::HandlingMode`; the
/// DSL owns the typed mirror so transitions can carry it without depending
/// on the shell's domain enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputLane {
    #[default]
    Queue,
    Steer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingSwitchTurnPhase {
    #[default]
    Requested,
    PendingForBoundary,
    ActiveFiniteOverride,
    ApplyingPersistentReconfigure,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingSwitchTurnTerminal {
    #[default]
    Denied,
    ConsumedAndRestored,
    PersistentReconfigureApplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingDenialReason {
    #[default]
    CapabilityPolicy,
    ApprovalRequiredButUnavailable,
    DeniedDuringApproval,
    ScopedOverrideConflict,
    RealtimeTransportConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingApprovalPhase {
    #[default]
    Pending,
    PresentedToUser,
    Approved,
    Denied,
    SurfaceDetached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingApprovalParentKind {
    #[default]
    SwitchTurn,
    ImageOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingImageOperationPhase {
    #[default]
    Requested,
    PlanResolved,
    ScopedOverrideActive,
    ProviderCallInFlight,
    ResultCommitted,
    RestoringScopedOverride,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingImageTerminal {
    #[default]
    Generated,
    Denied,
    EmptyResult,
    RefusedByProvider,
    SafetyFiltered,
    Failed,
    Cancelled,
    Timeout,
    ScopedRestoreFailed,
}

// Track-B (R5): declarative peer endpoint descriptor for the runtime
// DSL. Shape mirrors `meerkat_core::comms::TrustedPeerDescriptor`.
// The catalog DSL holds an identical type; the two are structurally
// equivalent so the schema validator sees consistent opaque struct
// shapes.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub signing_key: PeerSigningKey,
}

impl PeerEndpoint {
    pub fn new(
        name: impl Into<PeerName>,
        peer_id: impl Into<PeerId>,
        address: impl Into<PeerAddress>,
        signing_key: impl Into<PeerSigningKey>,
    ) -> Self {
        Self {
            name: name.into(),
            peer_id: peer_id.into(),
            address: address.into(),
            signing_key: signing_key.into(),
        }
    }
}

impl From<&meerkat_core::comms::TrustedPeerDescriptor> for PeerEndpoint {
    fn from(spec: &meerkat_core::comms::TrustedPeerDescriptor) -> Self {
        Self {
            name: PeerName(spec.name.as_str().to_owned()),
            peer_id: PeerId(spec.peer_id.to_string()),
            address: PeerAddress(spec.address.to_string()),
            signing_key: PeerSigningKey(spec.pubkey),
        }
    }
}

/// DSL-local carrier for the Ed25519 public signing key associated with a
/// peer endpoint. The MeerkatMachine owns this projection alongside the
/// endpoint identity atoms so trust reconciliation can install the exact
/// key into the comms trust store without shell-side defaults.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerSigningKey(pub [u8; 32]);

impl From<[u8; 32]> for PeerSigningKey {
    fn from(key: [u8; 32]) -> Self {
        Self(key)
    }
}

/// DSL-local newtype for a peer display name. Wraps the slug string
/// so the schema validator sees a stable opaque shape; mirrors
/// `meerkat_core::comms::PeerName` but avoids dragging the core
/// comms types into the DSL grammar.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerName(pub String);

impl<T: Into<String>> From<T> for PeerName {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl PeerName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// DSL-local newtype for the canonical peer routing id.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerId(pub String);

impl<T: Into<String>> From<T> for PeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl PeerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// DSL-local newtype for a peer transport endpoint URL.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerAddress(pub String);

impl<T: Into<String>> From<T> for PeerAddress {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl PeerAddress {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Ensure we keep the exact generated schema DSL body from the catalog source.

#[macro_export]
macro_rules! meerkat_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        machine! {
    machine MeerkatMachine {
        version: 1,
        rust: $rust_crate / $rust_module,

        state {
            lifecycle_phase: MeerkatPhase,
            session_id: Option<SessionId>,
            active_runtime_id: Option<AgentRuntimeId>,
            active_fence_token: Option<FenceToken>,
            current_run_id: Option<RunId>,
            pre_run_phase: Option<Enum<PreRunPhase>>,
            turn_phase: TurnPhase,
            primitive_kind: Option<Enum<TurnPrimitiveKind>>,
            admitted_content_shape: Option<Enum<ContentShape>>,
            vision_enabled: bool,
            image_tool_results_enabled: bool,
            tool_calls_pending: u64,
            pending_op_refs: Set<String>,
            barrier_operation_ids: Set<String>,
            has_barrier_ops: bool,
            barrier_satisfied: bool,
            boundary_count: u64,
            cancel_after_boundary: bool,
            terminal_outcome: Option<Enum<TurnTerminalOutcome>>,
            terminal_cause_kind: Option<Enum<TurnTerminalCauseKind>>,
            last_runtime_apply_failure_cause: Option<Enum<RuntimeApplyFailureCause>>,
            last_runtime_apply_failure_message: Option<String>,
            extraction_attempts: u64,
            max_extraction_retries: u64,
            llm_retry_attempt: u64,
            llm_retry_max_retries: u64,
            llm_retry_selected_delay_ms: u64,
            llm_retry_last_failure_kind: Option<Enum<LlmRetryFailureKind>>,
            silent_intent_overrides: Set<String>,

            // --- Model-routing / image-operation authority (Phase 1) ---
            model_routing_baseline_model: Option<String>,
            model_routing_baseline_realtime: Option<bool>,
            model_routing_topology_epoch: u64,
            model_routing_turn_override_id: Option<String>,
            model_routing_turn_request_id: Option<String>,
            model_routing_turn_target_model: Option<String>,
            model_routing_turn_realtime: Option<bool>,
            model_routing_turn_remaining_turns: Option<u64>,
            model_routing_operation_override_id: Option<String>,
            model_routing_operation_target_model: Option<String>,
            model_routing_operation_realtime: Option<bool>,
            model_routing_pending_switch_request_id: Option<String>,
            model_routing_pending_switch_target_model: Option<String>,
            model_routing_pending_switch_realtime: Option<bool>,
            model_routing_pending_switch_turns: Option<u64>,
            model_routing_pending_switch_phase: Option<Enum<RoutingSwitchTurnPhase>>,
            model_routing_switch_terminal: Map<String, Enum<RoutingSwitchTurnTerminal>>,
            model_routing_switch_denials: Map<String, Enum<RoutingDenialReason>>,
            model_routing_image_operation_phases: Map<String, Enum<RoutingImageOperationPhase>>,
            model_routing_image_operation_target_models: Map<String, String>,
            model_routing_image_operation_realtime: Map<String, bool>,
            model_routing_image_operation_requires_scoped_override: Map<String, bool>,
            model_routing_image_terminals: Map<String, Enum<RoutingImageTerminal>>,
            model_routing_image_terminal_payloads: Map<String, String>,
            model_routing_image_denials: Map<String, Enum<RoutingDenialReason>>,
            model_routing_approval_phases: Map<String, Enum<RoutingApprovalPhase>>,
            model_routing_approval_parent_kind: Map<String, Enum<RoutingApprovalParentKind>>,

            // --- Registration substate ---
            registration_phase: RegistrationPhase,

            // --- Comms drain substate ---
            drain_phase: DrainPhase,
            drain_mode: Option<DrainMode>,

            // --- Visibility substate ---
            //
            // `next_staged_visibility_revision` is the DSL-owned monotonic
            // counter that mints staged-revision tokens (dogma round 4,
            // wave 2b #12). `StageVisibilityFilter` / `StageDeferredNames` /
            // `RequestDeferredTools` increment it in their `update {}` and
            // write the new value into `staged_visibility_revision` in the
            // same atomic transition; the shell's `MachineToolVisibilityOwner`
            // reads the minted value back and applies it to its projection
            // rather than minting independently.
            next_staged_visibility_revision: u64,
            active_filter: ToolFilter,
            staged_filter: ToolFilter,
            active_visibility_revision: u64,
            staged_visibility_revision: u64,
            active_deferred_names: Set<String>,
            staged_deferred_names: Set<String>,
            active_deferred_authorities: Map<String, ToolVisibilityWitness>,
            staged_deferred_authorities: Map<String, ToolVisibilityWitness>,

            // --- Input lifecycle substate ---
            input_phases: Map<String, InputPhase>,
            input_terminal_kind: Map<String, InputTerminalKind>,
            input_superseded_by: Map<String, String>,
            input_aggregate_id: Map<String, String>,
            input_abandon_reason: Map<String, Enum<InputAbandonReason>>,
            input_abandon_attempt_count: Map<String, u64>,
            input_attempt_counts: Map<String, u64>,
            input_run_associations: Map<String, String>,
            input_boundary_sequences: Map<String, u64>,
            next_admission_seq: u64,
            input_admission_seq: Map<String, u64>,
            // Unified work-lane membership for admitted inputs. Mutual
            // exclusion between Queue and Steer is structural: an input
            // maps to exactly one `InputLane` value by construction.
            // Replaces the former `queue_lane`/`steer_lane` parallel sets.
            input_lane: Map<String, Enum<InputLane>>,

            // --- Ops lifecycle substate ---
            op_statuses: Map<String, Enum<OperationStatus>>,
            op_completion_seq: Map<String, u64>,
            // Terminal-outcome discriminant. Payload (result/error/reason)
            // rides on the companion `op_terminal_payload` map as JSON.
            op_terminal_outcomes: Map<String, Enum<OperationTerminalOutcomeKind>>,
            op_terminal_payload: Map<String, String>,
            op_kinds: Map<String, Enum<OperationKind>>,
            op_peer_ready: Map<String, bool>,
            op_progress_counts: Map<String, u64>,
            active_op_count: u64,
            wait_active: bool,
            wait_request_id: Option<WaitRequestId>,
            wait_operation_ids: Set<String>,
            wait_operation_id_tokens: Set<OperationId>,
            next_completion_seq: u64,

            // --- External tool surface substate ---
            known_surfaces: Set<String>,
            active_surfaces: Set<String>,
            visible_surfaces: Set<String>,
            surface_base_state: Map<String, Enum<ExternalToolSurfaceBaseState>>,
            surface_pending_op: Map<String, SurfacePendingOp>,
            surface_staged_op: Map<String, SurfaceStagedOp>,
            reload_staged_surfaces: Set<String>,
            surface_staged_intent_sequence: Map<String, u64>,
            next_staged_intent_sequence: u64,
            surface_pending_task_sequence: Map<String, u64>,
            next_pending_task_sequence: u64,
            surface_pending_lineage_sequence: Map<String, u64>,
            surface_inflight_calls: Map<String, u64>,
            surface_last_delta_operation: Map<String, Enum<ExternalToolSurfaceDeltaOperation>>,
            surface_last_delta_phase: Map<String, Enum<ExternalToolSurfaceDeltaPhase>>,
            snapshot_epoch: u64,
            snapshot_aligned_epoch: u64,
            surface_draining_since_ms: Map<String, u64>,
            surface_removal_timeout_at_ms: Map<String, u64>,
            surface_removal_applied_at_turn: Map<String, u64>,
            surface_phase: SurfacePhase,
            removal_timeout_ms: u64,

            // --- Realtime-attachment authority (per-session) ---
            realtime_intent_present: bool,
            realtime_binding_state: Enum<RealtimeBindingState>,
            realtime_binding_authority_epoch: Option<u64>,
            realtime_reattach_required: bool,
            realtime_next_authority_epoch: u64,

            // --- Realtime reconnect lifecycle ---
            //
            // The websocket shell supplies time observations and jittered
            // deadlines, but the DSL owns the reconnect cycle phase, attempt
            // count, retry deadline, exhaustion marker, and public status
            // projection.
            realtime_reconnect_cycle_state: Enum<RealtimeReconnectCycleState>,
            realtime_reconnect_attempt_count: u64,
            realtime_reconnect_next_retry_at_ms: Option<u64>,
            realtime_reconnect_deadline_at_ms: Option<u64>,

            // --- Live-topology reconfigure phase ---
            live_topology_phase: LiveTopologyPhase,

            // --- MCP server lifecycle ---
            //
            // Per-server connection state keyed by configured `McpServerId`.
            // Shell translates incoming transport events into DSL inputs;
            // `[MCP_PENDING]` system-notice toggle and tool-availability
            // filters read this map directly.
            mcp_server_states: Map<McpServerId, McpServerState>,

            // --- Peer interaction lifecycle (W1-A / issue #264) ---
            //
            // Outbound request lifecycle: each send records `Sent` under its
            // correlation id; progress and terminal response arrival rewrite
            // the state; timeouts mark `TimedOut`. Terminal transitions emit
            // `PeerInteractionCleanup` so the shell drops any subscriber /
            // stream channel keyed on the same correlation id.
            //
            // The registries the shell used to hand-maintain
            // (`subscriber_registry`, `interaction_stream_registry`) now
            // project off this map: channel live iff
            // `corr_id ∈ pending_peer_requests ∧ state ≠ terminal`.
            pending_peer_requests: Map<PeerCorrelationId, OutboundPeerRequestState>,
            // Inbound request lifecycle: mirror of the outbound map for
            // responses we owe back to remote peers. Receiver-side guard
            // prevents duplicate replies on the same correlation id.
            inbound_peer_requests: Map<PeerCorrelationId, InboundPeerRequestState>,

            // --- Session-context advancement (W2-E / issue #264) ---
            //
            // Monotonic watermark in milliseconds of the last canonical
            // session-context mutation the shell reported via
            // `AdvanceSessionContext`. Advancing transitions emit
            // `SessionContextAdvanced { updated_at_ms }` which the shell's
            // realtime projection consumer uses to drive a typed
            // `ProjectionFreshness` state machine instead of polling a
            // watch channel. Initialized to 0 so the first mutation always
            // advances.
            last_session_context_updated_at_ms: u64,

            // --- Interaction stream lifecycle (U6 / dogma #5) ---
            //
            // Authoritative reservation/attach/completion truth for any
            // interaction stream — covers both peer-request streams (keyed
            // on the same correlation id as `pending_peer_requests`) and
            // plain input streams (keyed on a fresh correlation id). The
            // shell-side `interaction_stream_registry` becomes a pure
            // projection of sender/receiver channels: the `state` field,
            // TTL bookkeeping, and CAS transitions all live here.
            //
            // The lifecycle is encoded as two disjoint sets — `Reserved`
            // streams are in `reserved_interaction_streams`; `Attached`
            // streams are in `attached_interaction_streams`. Terminal
            // states (`Completed`, `Expired`, `ClosedEarly`) leave both
            // sets and emit `InteractionStreamCleanup` so the shell drops
            // the matching channel entry. The
            // `interaction_stream_disjoint` invariant enforces that no
            // correlation id sits in both sets simultaneously, matching
            // the at-most-one-state-per-id semantics of a tagged-union
            // map without depending on map value comparison in DSL guards.
            reserved_interaction_streams: Set<PeerCorrelationId>,
            attached_interaction_streams: Set<PeerCorrelationId>,

            // --- Realtime product-turn lifecycle (U9 / dogma #4) ---
            //
            // Collapses the old shell locals (`product_turn_in_flight`,
            // `product_turn_committed`, `product_output_started`) into a
            // canonical five-phase closed set owned by the DSL. The
            // realtime-WS shell fires one input per lifecycle event and
            // reads `should_preempt_on_input` off the typed handle; no
            // shell-side bool tracking, no helper-local event matching.
            realtime_product_turn_phase: Enum<RealtimeProductTurnPhase>,

            // --- Realtime projection freshness (dogma round 2, U-C / dogma #1, #3, #13, #20) ---
            //
            // Canonical freshness truth for the realtime provider session's
            // projection relative to the canonical session state. Replaces
            // the shell-local `ProjectionFreshness` enum + observer queue
            // previously owned by `meerkat-rpc::realtime_ws`.
            //
            // `realtime_projection_freshness` carries the discriminant;
            // `realtime_projection_frontier_ms` holds the monotonic
            // watermark — the `baseline_ms` while `Clean`, or the pending
            // advance's `new_at_ms` while `StaleDeferred` / `StaleImmediate`.
            // Transitions are driven by four inputs:
            //   * `RealtimeProjectionAdvanceObserved { advanced_at_ms }` —
            //     fired on every `SessionContextAdvanced` observer tick;
            //     routes to `StaleDeferred` if the product turn is live,
            //     `StaleImmediate` otherwise.
            //   * `RealtimeProjectionRefreshed { observed_ms }` — fired
            //     after a successful provider-session refresh drain.
            //   * `RealtimeProjectionReset { baseline_ms }` — fired on
            //     product-session close / error / reconnect to re-seed the
            //     `Clean` baseline.
            //   * `ProductTurnTerminal` also folds in a
            //     `StaleDeferred → StaleImmediate` promotion so the DSL
            //     owns the turn-end-drain promotion directly.
            realtime_projection_freshness: Enum<RealtimeProjectionFreshness>,
            realtime_projection_frontier_ms: u64,

            // --- Realtime reconnect policy (dogma round 2, U-C / dogma #1, #3, #18, #20) ---
            //
            // Classifies what a clean provider-session close means for the
            // realtime channel's reconnect behavior. Replaces the shell-
            // local boolean pair (`client_has_submitted_input`,
            // `last_turn_terminally_completed`) that used to co-decide
            // `needs_reattach`. The shell reads this field directly at the
            // clean-close branch via
            // `RealtimeProductTurnHandle::reconnect_policy_on_clean_close`
            // and dispatches on the typed value.
            realtime_reconnect_policy: Enum<RealtimeReconnectPolicy>,

            // --- Peer-ingress transport capability ownership (W2-G / issue #264) ---
            //
            // Tracks which subsystem owns the peer-ingress transport
            // capability for this session. `Unattached` is the initial state;
            // session-standalone paths move to `SessionOwned`; mob
            // provisioners move to `MobOwned` (promotion from `SessionOwned`
            // is allowed, but silent downgrade from `MobOwned` →
            // `SessionOwned` is rejected by the `AttachSessionIngress`
            // guard — the s71 regression class closed structurally).
            // `peer_ingress_comms_runtime_id` and `peer_ingress_mob_id` are
            // populated iff the kind variant names them; the
            // `peer_ingress_owner_consistency` invariant enforces pairing.
            peer_ingress_owner_kind: Enum<PeerIngressOwnerKind>,
            peer_ingress_comms_runtime_id: Option<CommsRuntimeId>,
            peer_ingress_mob_id: Option<MobId>,

            // --- Supervisor-bridge authorization (Wave 3 D Row 21) ---
            //
            // Canonical authorization fact for the supervisor-bridge
            // command surface. Previously lived as an
            // `Option<AuthorizedSupervisorState>` on the comms drain
            // task's stack; the companion trust edge was router-owned, so
            // the authorization discriminant had split ownership. Now the
            // DSL owns both the kind and the full canonical binding
            // (`peer_id` + `name` + `address` + `epoch`); the trust edge
            // in the router stays in lock-step via shell-side
            // `add_trusted_peer` / `remove_trusted_peer` calls that only
            // run after the DSL mutator accepts the corresponding
            // `BindSupervisor` / `AuthorizeSupervisor` / `RevokeSupervisor`
            // transition. The `supervisor_binding_consistency` invariant
            // enforces that the companion fields are populated exactly
            // when `supervisor_binding_kind == Bound`.
            supervisor_binding_kind: Enum<SupervisorBindingKind>,
            supervisor_bound_name: Option<String>,
            supervisor_bound_peer_id: Option<String>,
            supervisor_bound_address: Option<String>,
            supervisor_bound_epoch: Option<u64>,

            // --- Track-B (R5): peer-projection state ---
            //
            // Identity-level wiring from MobMachine is projected onto
            // endpoint-level peer sets here. See the schema DSL
            // (`catalog::dsl::meerkat_machine`) for the full rationale;
            // the `effective` trust set is derived as
            // `direct_peer_endpoints ∪ mob_overlay_peer_endpoints` by
            // the comms reconciliation handler (Commit 4) on receipt
            // of `CommsTrustReconcileRequested`.
            //
            // `peer_projection_epoch` carries general effective-set
            // change freshness; `mob_overlay_epoch` is the overlay-
            // specific watermark the `stale_overlay_epoch` guard uses
            // so direct-endpoint mutations cannot lock out overlay
            // dispatches.
            local_endpoint: Option<PeerEndpoint>,
            direct_peer_endpoints: Set<PeerEndpoint>,
            mob_overlay_peer_endpoints: Set<PeerEndpoint>,
            peer_projection_epoch: u64,
            mob_overlay_epoch: u64,
        }

        init(Initializing) {
            session_id = None,
            active_runtime_id = None,
            active_fence_token = None,
            current_run_id = None,
            pre_run_phase = None,
            turn_phase = TurnPhase::Ready,
            primitive_kind = None,
            admitted_content_shape = None,
            vision_enabled = false,
            image_tool_results_enabled = false,
            tool_calls_pending = 0,
            pending_op_refs = EmptySet,
            barrier_operation_ids = EmptySet,
            has_barrier_ops = false,
            barrier_satisfied = false,
            boundary_count = 0,
            cancel_after_boundary = false,
            terminal_outcome = None,
            terminal_cause_kind = None,
            last_runtime_apply_failure_cause = None,
            last_runtime_apply_failure_message = None,
            extraction_attempts = 0,
            max_extraction_retries = 0,
            llm_retry_attempt = 0,
            llm_retry_max_retries = 0,
            llm_retry_selected_delay_ms = 0,
            llm_retry_last_failure_kind = None,
            silent_intent_overrides = EmptySet,
            model_routing_baseline_model = None,
            model_routing_baseline_realtime = None,
            model_routing_topology_epoch = 0,
            model_routing_turn_override_id = None,
            model_routing_turn_request_id = None,
            model_routing_turn_target_model = None,
            model_routing_turn_realtime = None,
            model_routing_turn_remaining_turns = None,
            model_routing_operation_override_id = None,
            model_routing_operation_target_model = None,
            model_routing_operation_realtime = None,
            model_routing_pending_switch_request_id = None,
            model_routing_pending_switch_target_model = None,
            model_routing_pending_switch_realtime = None,
            model_routing_pending_switch_turns = None,
            model_routing_pending_switch_phase = None,
            model_routing_switch_terminal = EmptyMap,
            model_routing_switch_denials = EmptyMap,
            model_routing_image_operation_phases = EmptyMap,
            model_routing_image_operation_target_models = EmptyMap,
            model_routing_image_operation_realtime = EmptyMap,
            model_routing_image_operation_requires_scoped_override = EmptyMap,
            model_routing_image_terminals = EmptyMap,
            model_routing_image_terminal_payloads = EmptyMap,
            model_routing_image_denials = EmptyMap,
            model_routing_approval_phases = EmptyMap,
            model_routing_approval_parent_kind = EmptyMap,
            // Registration substate
            registration_phase = RegistrationPhase::Queuing,
            // Comms drain substate
            drain_phase = DrainPhase::Inactive,
            drain_mode = None,
            // Visibility substate
            next_staged_visibility_revision = 0,
            active_filter = ToolFilter::All,
            staged_filter = ToolFilter::All,
            active_visibility_revision = 0,
            staged_visibility_revision = 0,
            active_deferred_names = EmptySet,
            staged_deferred_names = EmptySet,
            active_deferred_authorities = EmptyMap,
            staged_deferred_authorities = EmptyMap,
            // Input lifecycle substate
            input_phases = EmptyMap,
            input_terminal_kind = EmptyMap,
            input_superseded_by = EmptyMap,
            input_aggregate_id = EmptyMap,
            input_abandon_reason = EmptyMap,
            input_abandon_attempt_count = EmptyMap,
            input_attempt_counts = EmptyMap,
            input_run_associations = EmptyMap,
            input_boundary_sequences = EmptyMap,
            next_admission_seq = 0,
            input_admission_seq = EmptyMap,
            input_lane = EmptyMap,
            // Ops lifecycle substate
            op_statuses = EmptyMap,
            op_completion_seq = EmptyMap,
            op_terminal_outcomes = EmptyMap,
            op_terminal_payload = EmptyMap,
            op_kinds = EmptyMap,
            op_peer_ready = EmptyMap,
            op_progress_counts = EmptyMap,
            active_op_count = 0,
            wait_active = false,
            wait_request_id = None,
            wait_operation_ids = EmptySet,
            wait_operation_id_tokens = EmptySet,
            next_completion_seq = 0,
            known_surfaces = EmptySet,
            active_surfaces = EmptySet,
            visible_surfaces = EmptySet,
            surface_base_state = EmptyMap,
            surface_pending_op = EmptyMap,
            surface_staged_op = EmptyMap,
            reload_staged_surfaces = EmptySet,
            surface_staged_intent_sequence = EmptyMap,
            next_staged_intent_sequence = 0,
            surface_pending_task_sequence = EmptyMap,
            next_pending_task_sequence = 0,
            surface_pending_lineage_sequence = EmptyMap,
            surface_inflight_calls = EmptyMap,
            surface_last_delta_operation = EmptyMap,
            surface_last_delta_phase = EmptyMap,
            snapshot_epoch = 0,
            snapshot_aligned_epoch = 0,
            surface_draining_since_ms = EmptyMap,
            surface_removal_timeout_at_ms = EmptyMap,
            surface_removal_applied_at_turn = EmptyMap,
            surface_phase = SurfacePhase::Operating,
            removal_timeout_ms = 30000,
            realtime_intent_present = false,
            realtime_binding_state = RealtimeBindingState::Unbound,
            realtime_binding_authority_epoch = None,
            realtime_reattach_required = false,
            realtime_next_authority_epoch = 1,
            realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle,
            realtime_reconnect_attempt_count = 0,
            realtime_reconnect_next_retry_at_ms = None,
            realtime_reconnect_deadline_at_ms = None,
            live_topology_phase = LiveTopologyPhase::Idle,
            mcp_server_states = EmptyMap,
            pending_peer_requests = EmptyMap,
            inbound_peer_requests = EmptyMap,
            last_session_context_updated_at_ms = 0,
            reserved_interaction_streams = EmptySet,
            attached_interaction_streams = EmptySet,
            realtime_product_turn_phase = RealtimeProductTurnPhase::Idle,
            realtime_projection_freshness = RealtimeProjectionFreshness::Clean,
            realtime_projection_frontier_ms = 0,
            realtime_reconnect_policy = RealtimeReconnectPolicy::CleanExit,
            peer_ingress_owner_kind = PeerIngressOwnerKind::Unattached,
            peer_ingress_comms_runtime_id = None,
            peer_ingress_mob_id = None,
            supervisor_binding_kind = SupervisorBindingKind::Unbound,
            supervisor_bound_name = None,
            supervisor_bound_peer_id = None,
            supervisor_bound_address = None,
            supervisor_bound_epoch = None,
            // Track-B (R5): peer-projection state initialised empty.
            local_endpoint = None,
            direct_peer_endpoints = EmptySet,
            mob_overlay_peer_endpoints = EmptySet,
            peer_projection_epoch = 0,
            mob_overlay_epoch = 0,
        }

        terminal [Destroyed]

        phase MeerkatPhase {
            Initializing,
            Idle,
            Attached,
            Running,
            Retired,
            Stopped,
            Destroyed,
        }

        input MeerkatMachineInput {
            // Direct inputs
            RegisterSession { session_id: SessionId },
            UnregisterSession { session_id: SessionId },
            ReconfigureSessionLlmIdentity {
                previous_identity: SessionLlmIdentity,
                previous_visibility_state: SessionToolVisibilityState,
                previous_capability_surface: Option<SessionLlmCapabilitySurface>,
                previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
                target_identity: SessionLlmIdentity,
                target_capability_surface: SessionLlmCapabilitySurface,
                next_visibility_state: SessionToolVisibilityState,
                next_capability_base_filter: ToolFilter,
                next_active_visibility_revision: u64,
                tool_visibility_delta: SessionToolVisibilityDelta,
            },
            PrepareBindings { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: SessionId },
            SetPeerIngressContext { keep_alive: bool },
            NotifyDrainExited { reason: Enum<DrainExitReason> },
            InterruptCurrentRun,
            CancelAfterBoundary { reason: String },
            StagePersistentFilter { filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness> },
            RequestDeferredTools { authorities: Map<String, ToolVisibilityWitness> },
            PublishCommittedVisibleSet {
                active_filter: ToolFilter,
                staged_filter: ToolFilter,
                active_requested_deferred_names: Set<String>,
                staged_requested_deferred_names: Set<String>,
                active_deferred_authorities: Map<String, ToolVisibilityWitness>,
                staged_deferred_authorities: Map<String, ToolVisibilityWitness>,
                active_visibility_revision: u64,
                staged_visibility_revision: u64,
            },
            Recover,
            Retire { session_id: SessionId },
            Reset,
            StopRuntimeExecutor { reason: String },
            RuntimeExecutorExited,
            Destroy { session_id: SessionId },
            // Absorbed inputs
            EnsureSessionWithExecutor { session_id: SessionId },
            SetSilentIntents { session_id: SessionId, intents: Set<String> },
            ContainsSession { session_id: SessionId },
            SessionHasExecutor { session_id: SessionId },
            SessionHasComms { session_id: SessionId },
            OpsLifecycleRegistry { session_id: SessionId },
            InputState { session_id: SessionId, input_id: InputId },
            ListActiveInputs { session_id: SessionId },
            Abort { session_id: SessionId },
            AbortAll,
            Wait { session_id: SessionId },
            Ingest { runtime_id: AgentRuntimeId, work_id: WorkId, origin: Enum<WorkOrigin> },
            PublishEvent { kind: String },
            RuntimeState { runtime_id: String },
            RuntimeRealtimeAttachmentStatus { session_id: SessionId },
            ModelRoutingStatus { session_id: SessionId },
            SetModelRoutingBaseline { baseline_model: String, realtime_capable: bool },
            RequestFiniteSwitchTurn {
                request_id: String,
                target_model: String,
                turns: u64,
                target_realtime_capable: bool,
                requires_approval: bool,
                approval_available: bool,
                approval_denied: bool,
                realtime_detach_allowed: bool,
            },
            RequestUntilChangedSwitchTurn {
                request_id: String,
                target_model: String,
                target_realtime_capable: bool,
                requires_approval: bool,
                approval_available: bool,
                approval_denied: bool,
                realtime_detach_allowed: bool,
            },
            CompleteUntilChangedSwitchTurnReconfigure { request_id: String },
            AdmitModelRoutingAssistantTurn,
            BeginImageOperation {
                operation_id: String,
                target_model: String,
                target_realtime_capable: bool,
                requires_approval: bool,
                approval_available: bool,
                approval_denied: bool,
                realtime_detach_allowed: bool,
                requires_scoped_override: bool,
            },
            ActivateImageOperationOverride { operation_id: String, target_model: String, target_realtime_capable: bool },
            CompleteImageOperation { operation_id: String, terminal: Enum<RoutingImageTerminal>, terminal_payload: String },
            RestoreImageOperationOverride { operation_id: String },
            LoadBoundaryReceipt { runtime_id: String, sequence: u64 },
            AcceptWithCompletion { input_id: InputId, request_immediate_processing: bool, interrupt_yielding: bool, wake_if_idle: bool },
            AcceptWithoutWake { input_id: InputId },
            Prepare { session_id: SessionId, run_id: RunId },
            Commit { input_id: InputId, run_id: RunId },
            Fail { run_id: RunId },
            RollbackRun { run_id: RunId },
            Recycle,
            StartConversationRun {
                run_id: RunId,
                primitive_kind: Enum<TurnPrimitiveKind>,
                admitted_content_shape: Enum<ContentShape>,
                vision_enabled: bool,
                image_tool_results_enabled: bool,
                max_extraction_retries: u64,
            },
            StartImmediateAppend { run_id: RunId },
            StartImmediateContext { run_id: RunId },
            PrimitiveApplied,
            LlmReturnedToolCalls { tool_count: u64 },
            LlmReturnedTerminal,
            RegisterPendingOps { op_refs: Set<String>, barrier_operation_ids: Set<String> },
            ToolCallsResolved,
            OpsBarrierSatisfied { operation_ids: Set<String> },
            BoundaryContinue,
            BoundaryComplete,
            EnterExtraction { max_extraction_retries: u64 },
            ExtractionStart,
            ExtractionValidationPassed,
            ExtractionValidationFailed { error: String },
            RecoverableFailure {
                failure_kind: Enum<LlmRetryFailureKind>,
                retry_attempt: u64,
                max_retries: u64,
                selected_delay_ms: u64,
                error: String,
            },
            FatalFailure { terminal_cause_kind: Enum<TurnTerminalCauseKind>, error: String },
            RetryRequested { retry_attempt: u64 },
            CancelNow,
            RequestCancelAfterBoundary,
            CancellationObserved,
            AcknowledgeTerminal { outcome: Enum<TurnTerminalOutcome> },
            TurnLimitReached,
            BudgetExhausted,
            TimeBudgetExceeded,
            ForceCancelNoRun,
            RunCompleted { run_id: RunId },
            RunFailed {
                run_id: RunId,
                runtime_apply_failure_cause: Option<Enum<RuntimeApplyFailureCause>>,
                runtime_apply_failure_message: Option<String>,
                terminal_outcome: Enum<TurnTerminalOutcome>,
                terminal_cause_kind: Enum<TurnTerminalCauseKind>,
                error: String,
            },
            RunCancelled { run_id: RunId },
            // Input lifecycle inputs
            RecoverInputLifecycle {
                input_id: String,
                phase: Enum<InputPhase>,
                terminal_kind: Option<Enum<InputTerminalKind>>,
                superseded_by: Option<String>,
                aggregate_id: Option<String>,
                abandon_reason: Option<Enum<InputAbandonReason>>,
                abandon_attempt_count: u64,
                attempt_count: u64,
                run_id: Option<String>,
                boundary_sequence: Option<u64>,
                lane: Option<Enum<InputLane>>,
            },
            QueueAccepted { input_id: String },
            SteerAccepted { input_id: String },
            ChangeLane { input_id: String, new_lane: Enum<InputLane> },
            StageForRun { input_id: String, run_id: String },
            IncrementAttemptCount { input_id: String },
            RollbackStaged { input_id: String, lane: Enum<InputLane> },
            MarkApplied { input_id: String },
            MarkAppliedPendingConsumption { input_id: String },
            ConsumeInput { input_id: String },
            ConsumeOnAccept { input_id: String },
            SupersedeInput { input_id: String, superseded_by: String },
            CoalesceInput { input_id: String, aggregate_id: String },
            AbandonInput {
                input_id: String,
                reason: Enum<InputAbandonReason>,
                attempt_count: u64,
            },
            RecordBoundarySeq { input_id: String, seq: u64 },
            // Ops lifecycle inputs.
            // Terminal transitions carry a typed outcome discriminant plus
            // an opaque `payload` string — the inner payload of the domain
            // `OperationTerminalOutcome` encoded as JSON by the shell. The
            // DSL does not parse the payload; it only tracks the closed-set
            // discriminant so guards can reason about it.
            RegisterOp { operation_id: String, kind: Enum<OperationKind> },
            StartOp { operation_id: String },
            CompleteOp { operation_id: String, outcome: Enum<OperationTerminalOutcomeKind>, payload: String },
            FailOp { operation_id: String, outcome: Enum<OperationTerminalOutcomeKind>, payload: String },
            CancelOp { operation_id: String, outcome: Enum<OperationTerminalOutcomeKind>, payload: String },
            AbortOp { operation_id: String, outcome: Enum<OperationTerminalOutcomeKind>, payload: String },
            PeerReadyOp { operation_id: String },
            ProgressReportedOp { operation_id: String },
            RetireRequestedOp { operation_id: String },
            RetireCompletedOp { operation_id: String, outcome: Enum<OperationTerminalOutcomeKind>, payload: String },
            TerminateOp { operation_id: String, outcome: Enum<OperationTerminalOutcomeKind>, payload: String },
            RequestWaitAll { wait_request_id: WaitRequestId, operation_ids: Set<String>, operation_id_tokens: Set<OperationId> },
            SatisfyWaitAll { wait_request_id: WaitRequestId, operation_id_tokens: Set<OperationId> },
            CancelWaitAll,
            // Comms drain inputs
            SpawnDrain { mode: DrainMode },
            StopDrain,
            DrainExitedClean,
            DrainExitedRespawnable,
            // Visibility inputs
            // Dogma round 4, wave 2b #12: `StageVisibilityFilter` no longer
            // accepts a revision parameter — the DSL mints it via
            // `next_staged_visibility_revision` in the transition's update.
            StageVisibilityFilter { filter: ToolFilter },
            CommitVisibilityFilter { filter: ToolFilter, revision: u64 },
            StageDeferredNames { names: Set<String> },
            CommitDeferredNames { authorities: Map<String, ToolVisibilityWitness> },
            // Sync the DSL monotonic staged-revision counter to at least the
            // max of externally-installed active/staged revisions. Fired
            // from the shell's `replace_visibility_state` path (recovery
            // and LLM-reconfigure hot-swap), so subsequent
            // `StageVisibilityFilter` / `StageDeferredNames` mints advance
            // from the already-durable high-water mark rather than 0 —
            // preserving `max(active, staged)`-advance across external
            // state installs.
            SyncVisibilityRevisions {
                active_revision: u64,
                staged_revision: u64,
                active_deferred_names: Set<String>,
                staged_deferred_names: Set<String>,
                active_deferred_authorities: Map<String, ToolVisibilityWitness>,
                staged_deferred_authorities: Map<String, ToolVisibilityWitness>,
            },
            SurfaceRegister { surface_id: String },
            SurfaceStageAdd { surface_id: String, now_ms: u64 },
            SurfaceStageRemove { surface_id: String, now_ms: u64 },
            SurfaceStageReload { surface_id: String, now_ms: u64 },
            SurfaceApplyBoundary {
                surface_id: String,
                now_ms: u64,
                staged_intent_sequence: u64,
                applied_at_turn: u64,
            },
            SurfaceMarkPendingSucceeded {
                surface_id: String,
                pending_task_sequence: u64,
                staged_intent_sequence: u64,
            },
            SurfaceMarkPendingFailed {
                surface_id: String,
                pending_task_sequence: u64,
                staged_intent_sequence: u64,
                cause: Enum<ExternalToolSurfaceFailureCause>,
            },
            SurfaceCallStarted { surface_id: String },
            SurfaceCallFinished { surface_id: String },
            SurfaceFinalizeRemovalClean { surface_id: String },
            SurfaceFinalizeRemovalForced { surface_id: String },
            SurfaceSnapshotAligned { epoch: u64 },
            SurfaceShutdown,
            // Realtime-attachment inputs.
            ProjectRealtimeIntent { present: bool },
            BeginRealtimeBinding,
            ReplaceRealtimeBinding,
            DetachRealtimeBinding,
            RequireRealtimeReattach,
            RequireRealtimeReattachForAuthority { authority_epoch: u64 },
            PublishRealtimeSignal { authority_epoch: u64, next_binding_state: Enum<RealtimeBindingState> },
            // Reconnect retry lifecycle. The websocket shell supplies
            // millis-since-epoch retry deadlines from its clock/jitter source;
            // the DSL owns cycle lifetime, attempt increments, exhaustion, and
            // public status projection.
            BeginRealtimeReconnectCycle {
                next_retry_at_ms: Option<u64>,
                deadline_at_ms: Option<u64>,
            },
            ScheduleRealtimeReconnectRetry { next_retry_at_ms: Option<u64> },
            ExhaustRealtimeReconnectCycle,
            ClearRealtimeReconnectProgress,
            // Live-topology reconfigure inputs.
            BeginLiveTopologyReconfigure { authority_epoch: u64 },
            MarkLiveTopologyDetached,
            ApplyLiveTopologyIdentity,
            ApplyLiveTopologyVisibility,
            CompleteLiveTopology,
            AbortLiveTopologyBeforeDetach,
            FailLiveTopologyAfterDetach,
            // MCP server lifecycle inputs. Shell fires these when transport
            // state changes; each rewrites the server's slot in
            // `mcp_server_states` and emits `McpServerStateChanged`.
            McpServerConnectPending { server_id: McpServerId },
            McpServerConnected { server_id: McpServerId },
            McpServerFailed { server_id: McpServerId, error: String },
            McpServerDisconnected { server_id: McpServerId },
            McpServerReload { server_id: McpServerId },
            // Peer interaction lifecycle inputs (W1-A). Shell fires these on
            // outbound send, response arrival (progress or terminal),
            // timeout, inbound request arrival, and inbound reply completion.
            PeerRequestSent { corr_id: PeerCorrelationId, to: String },
            PeerResponseProgressArrived { corr_id: PeerCorrelationId },
            PeerResponseTerminalArrived { corr_id: PeerCorrelationId, disposition: PeerTerminalDisposition },
            PeerRequestTimedOut { corr_id: PeerCorrelationId },
            PeerRequestReceived { corr_id: PeerCorrelationId },
            PeerResponseReplied { corr_id: PeerCorrelationId },
            // Session-context advancement input (W2-E). Shell fires this at every
            // site that mutates canonical session truth (prompt append, external
            // content injection, tool-result append, external assistant output,
            // runtime-system-context append). The transition records
            // `last_session_context_updated_at_ms` and emits
            // `SessionContextAdvanced` so external projection consumers (realtime
            // provider session) refresh from a typed effect instead of polling a
            // watch channel. Monotonic: rejected when `updated_at_ms` is not
            // strictly greater than the last advance we recorded.
            AdvanceSessionContext { updated_at_ms: u64 },
            // Interaction stream lifecycle inputs (U6 / dogma #5). Shell fires
            // these on reservation, attach, terminal completion, TTL expiry,
            // and consumer drop. See the `interaction_streams` state field
            // commentary for invariants.
            InteractionStreamReserved { corr_id: PeerCorrelationId },
            InteractionStreamAttached { corr_id: PeerCorrelationId },
            InteractionStreamCompleted { corr_id: PeerCorrelationId },
            InteractionStreamExpired { corr_id: PeerCorrelationId },
            InteractionStreamClosedEarly { corr_id: PeerCorrelationId },
            // Realtime product-turn lifecycle inputs (U9 / dogma #4). The
            // realtime-WS shell fires one of these per observed provider-
            // session event (input accepted, TurnCommitted, output delta /
            // tool call, interrupted, logical turn completed); idempotent
            // transitions are guard-rejected and surfaced as `Ok(false)`
            // by the handle.
            ProductTurnInFlight,
            ProductTurnCommitted,
            ProductOutputStarted,
            ProductTurnInterrupted,
            ProductTurnTerminal,
            // Realtime projection freshness inputs (dogma round 2, U-C /
            // dogma #1, #3, #13, #20). The realtime-WS shell fires
            // `RealtimeProjectionAdvanceObserved` on every
            // `SessionContextAdvanced` observer tick, `RealtimeProjectionRefreshed`
            // after a successful provider-session rebuild, and
            // `RealtimeProjectionReset` on product-session close / error /
            // reconnect. The DSL decides whether each advance lands as
            // `StaleDeferred` (turn live) or `StaleImmediate` (turn idle).
            RealtimeProjectionAdvanceObserved { advanced_at_ms: u64 },
            RealtimeProjectionRefreshed { observed_ms: u64 },
            RealtimeProjectionReset { baseline_ms: u64 },
            // Realtime reconnect-policy inputs (dogma round 2, U-C / dogma
            // #1, #3, #18, #20). `ClassifyRealtimeClientInputSubmitted` fires
            // when the client's input chunk is accepted by the provider
            // session, flipping the policy to `ReattachAndRecover`.
            // `ClassifyRealtimeMidTurnActivity` fires on a provider-issued
            // tool call inside a live turn (mid-work signal), also routing to
            // `ReattachAndRecover`. `ClassifyRealtimeTurnTerminated` fires
            // on a logical turn terminal, routing to `CleanExit` (the
            // session delivered what the client asked for).
            ClassifyRealtimeClientInputSubmitted,
            ClassifyRealtimeMidTurnActivity,
            ClassifyRealtimeTurnTerminated,
            // Peer-ingress transport capability ownership (W2-G).
            //
            // `AttachSessionIngress` only succeeds from `Unattached`:
            // transitioning from `MobOwned` back to `SessionOwned` would be a
            // silent transport downgrade and is rejected structurally.
            // `AttachMobIngress` allows promotion from `Unattached` or
            // `SessionOwned` (mob provisioning takes over a session-attached
            // drain). `DetachIngress` clears any active ownership.
            AttachSessionIngress { comms_runtime_id: CommsRuntimeId },
            AttachMobIngress { comms_runtime_id: CommsRuntimeId, mob_id: MobId },
            DetachIngress,
            // Supervisor-bridge authorization (Wave 3 D Row 21).
            //
            // `BindSupervisor` establishes the initial binding from the
            // `Unbound` state. `AuthorizeSupervisor` rotates an already
            // `Bound` binding — the shell enforces the "new supervisor must
            // be authorized by the current supervisor" gate via
            // sender-authentication on the incoming request before firing
            // this input. `RevokeSupervisor` tears the binding down and
            // returns to `Unbound`; the `epoch` and `peer_id` arguments
            // must match the current binding so a stale revoke cannot
            // clear a rotated binding.
            BindSupervisor {
                name: String,
                peer_id: String,
                address: String,
                epoch: u64,
            },
            AuthorizeSupervisor {
                name: String,
                peer_id: String,
                address: String,
                epoch: u64,
            },
            RevokeSupervisor {
                peer_id: String,
                epoch: u64,
            },

            // Supervisor-trust-edge feedback inputs (C-F2 / wave-d D-d).
            //
            // Feedback acks for the `supervisor_trust_publish` /
            // `supervisor_trust_revoke` handoff protocols hosted on the
            // canonical `MeerkatMachine` producer effects. The realising shell
            // (`comms_drain::try_handle_supervisor_bridge_command`) calls
            // `Router::add_trusted_peer` / `remove_trusted_peer` after the
            // `BindSupervisor` / `AuthorizeSupervisor` / `RevokeSupervisor`
            // DSL commit, then stages one of these four inputs carrying
            // the `epoch` observed on the producer effect. The
            // transitions guard on `peer_id` and `epoch` matching the
            // current `supervisor_bound_*` binding, so a stale ack for
            // epoch `N - 1` arriving after the binding has rotated to
            // epoch `N` is rejected by the DSL without clearing the
            // outstanding obligation.
            SupervisorTrustEdgePublished {
                peer_id: String,
                epoch: u64,
            },
            SupervisorTrustEdgePublishFailed {
                peer_id: String,
                epoch: u64,
                reason: String,
            },
            SupervisorTrustEdgeRevoked {
                peer_id: String,
                epoch: u64,
            },
            SupervisorTrustEdgeRevokeFailed {
                peer_id: String,
                epoch: u64,
                reason: String,
            },

            // Track-B (R5) peer-projection inputs. See catalog DSL for
            // full rationale. Rejected in `Initializing`, `Retired`,
            // `Stopped`, `Destroyed`.
            PublishLocalEndpoint {
                endpoint: PeerEndpoint,
            },
            ClearLocalEndpoint,
            AddDirectPeerEndpoint {
                endpoint: PeerEndpoint,
            },
            RemoveDirectPeerEndpoint {
                endpoint: PeerEndpoint,
            },
            ApplyMobPeerOverlay {
                epoch: u64,
                endpoints: Set<PeerEndpoint>,
            },
        }

        surface_only [
            ContainsSession,
            SessionHasExecutor,
            SessionHasComms,
            OpsLifecycleRegistry,
            InputState,
            ListActiveInputs,
            RuntimeState,
            RuntimeRealtimeAttachmentStatus,
            ModelRoutingStatus,
            LoadBoundaryReceipt
        ]

        signal MeerkatMachineSignal {
            Initialize,
            BoundaryApplied { revision: u64 },
            DrainQueuedRun { run_id: RunId },
            ClassifyExternalEnvelope {
                item_id: String,
                from_peer: String,
                envelope_kind: Enum<PeerIngressEnvelopeClass>,
                request_intent: String,
                lifecycle_kind: Enum<PeerIngressLifecycleClass>,
                lifecycle_peer_param: Option<String>,
                response_status: Enum<PeerIngressResponseStatus>,
                in_reply_to: String,
            },
            ClassifyPlainEvent { source_name: String },
            EnsureDrainRunning,
        }

        effect MeerkatMachineEffect {
            RuntimeBound { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            TurnRunStarted { run_id: RunId },
            TurnBoundaryApplied { run_id: RunId, boundary_sequence: u64 },
            TurnRunCompleted { run_id: RunId, outcome: Enum<TurnTerminalOutcome> },
            // `error` is a display message projection. The terminal cause is
            // carried by `terminal_cause_kind`, not inferred from this string.
            TurnRunFailed {
                run_id: RunId,
                terminal_cause_kind: Enum<TurnTerminalCauseKind>,
                error: String
            },
            TurnRunCancelled { run_id: RunId, reason: Enum<TurnCancellationReason> },
            TurnCheckCompaction,
            RequestCancellationAtBoundary,
            WakeInterrupt,
            CommittedVisibleSetPublished { revision: u64 },
            // `kind` is a closed classifier of runtime lifecycle markers;
            // `detail` stays `String` because it's a free-form diagnostic
            // message paired with the kind (e.g. "runtime executor stopped",
            // "runtime recovered"). Detail strings are not matched on by
            // consumers — they surface through structured logging / traces
            // as human-readable context.
            RuntimeNotice { kind: Enum<RuntimeNoticeKind>, detail: String },
            RuntimeEffectFact { kind: Enum<RuntimeEffectKind>, reason: String },
            ModelRoutingStatusChanged { topology_epoch: u64 },
            SwitchTurnDenied { request_id: String, reason: Enum<RoutingDenialReason> },
            SwitchTurnPersistentReconfigureRequested { request_id: String, target_model: String },
            SwitchTurnFiniteOverrideActivated { request_id: String, target_model: String, turns_remaining: u64 },
            SwitchTurnFiniteOverrideRestored { request_id: String },
            ImageOperationPhaseChanged { operation_id: String, phase: Enum<RoutingImageOperationPhase> },
            ImageOperationDenied { operation_id: String, reason: Enum<RoutingDenialReason> },
            ModelRoutingApprovalTerminalized { approval_id: String, phase: Enum<RoutingApprovalPhase> },
            // Absorbed effects
            ResolveAdmission,
            SubmitAdmittedIngressEffect,
            SubmitRunPrimitive,
            ResolveCompletionAsTerminated,
            ApplyControlPlaneCommand,
            InitiateRecycle,
            IngressAccepted,
            PostAdmissionSignal { signal: Enum<PostAdmissionSignalKind> },
            ReadyForRun,
            InputLifecycleNotice,
            CompletionResolved,
            IngressNotice,
            SilentIntentApplied,
            CheckCompaction,
            RecordTerminalOutcome,
            RecordRunAssociation,
            RecordBoundarySequence,
            SubmitOpEvent { operation_id: String },
            NotifyOpWatcher { operation_id: String },
            ExposeOperationPeer { operation_id: String },
            RetainTerminalRecord { operation_id: String },
            EvictCompletedRecord { operation_id: String },
            CompletionProduced { seq: u64, operation_id: OperationId, kind: OperationKind },
            WaitAllSatisfied { wait_request_id: WaitRequestId, operation_ids: Set<OperationId> },
            CollectCompletedResult,
            EnqueueClassifiedEntry,
            PeerIngressClassified {
                class: Enum<PeerIngressInputClass>,
                kind: Enum<PeerIngressAdmittedKind>,
                auth: Enum<PeerIngressAuthClass>,
                lifecycle_kind: Option<Enum<PeerIngressLifecycleClass>>,
                lifecycle_peer: Option<String>,
                request_id: Option<String>,
                response_terminality: Option<Enum<PeerIngressResponseTerminality>>,
            },
            SpawnDrainTask,
            // `surface_id` stays `String`: it's an opaque surface identity,
            // not a closed classifier. `operation` is the same typed
            // discriminant used on `EmitExternalToolDelta` and on the
            // `surface_last_delta_operation` state map.
            ScheduleSurfaceCompletion {
                surface_id: String,
                operation: Enum<ExternalToolSurfaceDeltaOperation>,
                pending_task_sequence: u64,
                staged_intent_sequence: u64,
                applied_at_turn: u64,
            },
            RefreshVisibleSurfaceSet { snapshot_epoch: u64 },
            // `surface_id` stays `String` (opaque surface identity). Both
            // `operation` and `phase` are closed classifiers mirrored from
            // `meerkat-core::tool_scope`.
            EmitExternalToolDelta {
                surface_id: String,
                operation: Enum<ExternalToolSurfaceDeltaOperation>,
                phase: Enum<ExternalToolSurfaceDeltaPhase>,
                cause: Option<Enum<ExternalToolSurfaceFailureCause>>,
            },
            CloseSurfaceConnection { surface_id: String },
            RejectSurfaceCall { surface_id: String, cause: Enum<ExternalToolSurfaceFailureCause> },
            PublishSupervisorTrustEdge {
                peer_id: String,
                name: String,
                address: String,
                signing_public_key: Option<String>,
                epoch: u64,
            },
            RevokeSupervisorTrustEdge { peer_id: String, epoch: u64 },
            // Realtime-attachment effects.
            RealtimeIntentProjected { present: bool },
            RealtimeBindingRotated { authority_epoch: u64 },
            // Reconnect-progress state changed. Shell consumers (e.g.
            // observability pipelines) can subscribe; production RPC/MCP
            // `realtime/status` responders read the state fields directly.
            RealtimeReconnectProgressProjected {
                attempt_count: u64,
                next_retry_at_ms: Option<u64>,
                deadline_at_ms: Option<u64>,
            },
            // Live-topology reconfigure effects.
            LiveTopologyPhaseChanged,
            // MCP server lifecycle effects.
            McpServerStateChanged { server_id: McpServerId, new_state: McpServerState },
            McpServerReloadRequested { server_id: McpServerId },
            // Peer interaction lifecycle effects (W1-A). Emitted on every
            // lifecycle-advancing transition so the shell can update
            // subscriber/stream projections; the cleanup variant is the
            // authoritative signal to drop channels keyed on `corr_id`.
            PeerInteractionStateChanged { corr_id: PeerCorrelationId, new_state: OutboundPeerRequestState },
            PeerInteractionCleanup { corr_id: PeerCorrelationId },
            InboundPeerInteractionStateChanged { corr_id: PeerCorrelationId, new_state: InboundPeerRequestState },
            // Session-context advancement effect (W2-E / issue #264). Emitted
            // on every transition that advances canonical session-context
            // truth. Replaces the hand-wired `projection_refresh_rx` polling
            // channel in the realtime projection consumer — consumers
            // subscribe to this typed effect via an observer installed on the
            // session's DSL handle. `updated_at_ms` is monotonic; observers
            // use it as the freshness baseline for their typed
            // `ProjectionFreshness` state.
            SessionContextAdvanced { updated_at_ms: u64 },
            // Interaction stream lifecycle effects (U6 / dogma #5). Emitted on
            // every state-advancing transition for observers/diagnostics; the
            // cleanup variant is the authoritative signal to drop the
            // shell-side channel projection.
            InteractionStreamStateChanged { corr_id: PeerCorrelationId, new_state: InteractionStreamState },
            InteractionStreamCleanup { corr_id: PeerCorrelationId },
            // Realtime product-turn lifecycle effect (U9 / dogma #4). Emitted
            // on every phase-advancing transition so the realtime-WS shell
            // can log / observe phase changes without polling the handle.
            // The realtime-WS dispatch loop reads the typed handle directly
            // for preempt decisions; this effect is currently informational.
            RealtimeProductTurnPhaseChanged { new_phase: Enum<RealtimeProductTurnPhase> },
            // Realtime projection freshness + reconnect policy change
            // effects (dogma round 2, U-C / dogma #1, #3, #13, #18, #20).
            // Emitted on every DSL-owned projection-state / policy advance
            // so the shell can trace transitions. The realtime-WS dispatcher
            // reads the typed handle directly for drain + close-branch
            // decisions; these effects are informational.
            RealtimeProjectionFreshnessChanged {
                new_freshness: Enum<RealtimeProjectionFreshness>,
                frontier_ms: u64,
            },
            RealtimeReconnectPolicyChanged { new_policy: Enum<RealtimeReconnectPolicy> },
            // Track-B (R5) peer-projection effects.
            LocalEndpointChanged { endpoint: Option<PeerEndpoint> },
            PeerProjectionChanged { peer_projection_epoch: u64 },
            CommsTrustReconcileRequested { peer_projection_epoch: u64 },
        }

        // =====================================================================
        // Effect dispositions
        // =====================================================================

        disposition RuntimeBound => routed [MobMachine],
        disposition RuntimeRetired => routed [MobMachine],
        disposition RuntimeDestroyed => routed [MobMachine],
        disposition TurnRunStarted => local,
        disposition TurnBoundaryApplied => local,
        disposition TurnRunCompleted => local,
        disposition TurnRunFailed => local,
        disposition TurnRunCancelled => local,
        disposition TurnCheckCompaction => local,
        disposition RequestCancellationAtBoundary => local,
        disposition WakeInterrupt => local,
        disposition CommittedVisibleSetPublished => external,
        disposition RuntimeNotice => external,
        disposition RuntimeEffectFact => local,
        disposition ModelRoutingStatusChanged => external,
        disposition SwitchTurnDenied => external,
        disposition SwitchTurnPersistentReconfigureRequested => local,
        disposition SwitchTurnFiniteOverrideActivated => local,
        disposition SwitchTurnFiniteOverrideRestored => local,
        disposition ImageOperationPhaseChanged => external,
        disposition ImageOperationDenied => external,
        disposition ModelRoutingApprovalTerminalized => external,
        // Absorbed effect dispositions
        disposition ResolveAdmission => local,
        disposition SubmitAdmittedIngressEffect => local,
        disposition SubmitRunPrimitive => local,
        disposition ResolveCompletionAsTerminated => local,
        disposition ApplyControlPlaneCommand => local,
        disposition InitiateRecycle => local,
        disposition IngressAccepted => external,
        disposition PostAdmissionSignal => local,
        disposition ReadyForRun => local,
        disposition InputLifecycleNotice => external,
        disposition CompletionResolved => local,
        disposition IngressNotice => external,
        disposition SilentIntentApplied => external,
        disposition CheckCompaction => local,
        disposition RecordTerminalOutcome => local,
        disposition RecordRunAssociation => local,
        disposition RecordBoundarySequence => local,
        disposition SubmitOpEvent => local,
        disposition NotifyOpWatcher => local,
        disposition ExposeOperationPeer => local,
        disposition RetainTerminalRecord => local,
        disposition EvictCompletedRecord => local,
        disposition CompletionProduced => local,
        disposition WaitAllSatisfied => external handoff ops_barrier_satisfaction,
        disposition CollectCompletedResult => local,
        disposition EnqueueClassifiedEntry => local,
        disposition PeerIngressClassified => local,
        disposition SpawnDrainTask => local,
        disposition ScheduleSurfaceCompletion => external handoff surface_completion,
        disposition RefreshVisibleSurfaceSet => external handoff surface_snapshot_alignment,
        disposition EmitExternalToolDelta => external,
        disposition CloseSurfaceConnection => local,
        disposition RejectSurfaceCall => external,
        disposition PublishSupervisorTrustEdge => external handoff supervisor_trust_publish,
        disposition RevokeSupervisorTrustEdge => external handoff supervisor_trust_revoke,
        disposition RealtimeIntentProjected => external,
        disposition RealtimeBindingRotated => external,
        disposition RealtimeReconnectProgressProjected => external,
        disposition LiveTopologyPhaseChanged => external,
        disposition McpServerStateChanged => external,
        disposition McpServerReloadRequested => external,
        disposition PeerInteractionStateChanged => external,
        disposition PeerInteractionCleanup => external,
        disposition InboundPeerInteractionStateChanged => external,
        disposition SessionContextAdvanced => external,
        disposition InteractionStreamStateChanged => external,
        disposition InteractionStreamCleanup => external,
        disposition RealtimeProductTurnPhaseChanged => external,
        disposition RealtimeProjectionFreshnessChanged => external,
        disposition RealtimeReconnectPolicyChanged => external,
        disposition LocalEndpointChanged => external,
        disposition PeerProjectionChanged => external,
        disposition CommsTrustReconcileRequested => external,

        // =====================================================================
        // Helpers
        // =====================================================================

        helper deferred_authority_has_identity(witness: ToolVisibilityWitness) -> bool {
            witness.len() > 0
        }

        helper deferred_authorities_have_identity(
            names: Set<String>,
            witnesses: Map<String, ToolVisibilityWitness>
        ) -> bool {
            for_all(requested_name in names,
                deferred_authority_has_identity(witnesses.get_cloned(requested_name).get("value")))
        }

        // =====================================================================
        // Invariants
        // =====================================================================

        invariant fence_requires_bound_runtime {
            self.active_fence_token == None || self.active_runtime_id != None
        }

        invariant running_has_current_run {
            self.lifecycle_phase != Phase::Running || self.current_run_id != None
        }

        invariant current_run_only_while_running_or_retired {
            self.current_run_id == None
            || self.lifecycle_phase == Phase::Running
            || self.lifecycle_phase == Phase::Retired
        }

        invariant staged_surface_ops_are_known_and_sequenced {
            for_all(surface_id in self.surface_staged_op.keys(),
                self.known_surfaces.contains(surface_id)
                && self.surface_staged_intent_sequence.contains_key(surface_id))
        }

        invariant staged_reload_surfaces_are_active {
            for_all(surface_id in self.reload_staged_surfaces,
                self.active_surfaces.contains(surface_id))
        }

        // Realtime binding state and authority epoch must stay in lockstep.
        // Unbound iff no epoch; any active binding state must carry Some(epoch).
        // Prevents Unbound+Some(epoch) and BindingReady+None from being
        // representable as a TLC-enforceable fact (DSL-native substitute for
        // a typed sum).
        invariant realtime_binding_epoch_consistency {
            (self.realtime_binding_state == RealtimeBindingState::Unbound)
            == (self.realtime_binding_authority_epoch == None)
        }

        // Peer-ingress owner companion fields must stay in lockstep with the
        // kind variant. `Unattached` carries no companions; `SessionOwned`
        // carries only a comms runtime id; `MobOwned` carries both. The DSL
        // encodes the tagged-union discipline across three fields so silent
        // transitions that leave the comms runtime id behind (the s71
        // regression class) cannot serialize.
        invariant peer_ingress_owner_consistency {
            (self.peer_ingress_owner_kind == PeerIngressOwnerKind::Unattached
                && self.peer_ingress_comms_runtime_id == None
                && self.peer_ingress_mob_id == None)
            || (self.peer_ingress_owner_kind == PeerIngressOwnerKind::SessionOwned
                && self.peer_ingress_comms_runtime_id != None
                && self.peer_ingress_mob_id == None)
            || (self.peer_ingress_owner_kind == PeerIngressOwnerKind::MobOwned
                && self.peer_ingress_comms_runtime_id != None
                && self.peer_ingress_mob_id != None)
        }

        // Supervisor-binding tagged-union discipline (Wave 3 D Row 21).
        // `Unbound` carries no companions; `Bound` carries all four
        // (`name`, `peer_id`, `address`, `epoch`). A half-populated binding
        // is structurally unrepresentable — the split-ownership regression
        // class is closed by construction.
        invariant supervisor_binding_consistency {
            (self.supervisor_binding_kind == SupervisorBindingKind::Unbound
                && self.supervisor_bound_name == None
                && self.supervisor_bound_peer_id == None
                && self.supervisor_bound_address == None
                && self.supervisor_bound_epoch == None)
            || (self.supervisor_binding_kind == SupervisorBindingKind::Bound
                && self.supervisor_bound_name != None
                && self.supervisor_bound_peer_id != None
                && self.supervisor_bound_address != None
                && self.supervisor_bound_epoch != None)
        }


        // =====================================================================
        // Direct transitions
        // =====================================================================

        // 1. Initialize: Initializing → Idle
        transition Initialize {
            on signal Initialize
            guard { self.lifecycle_phase == Phase::Initializing }
            update {}
            to Idle
        }

        // 2. RegisterSession: per-phase self-loop, no guard
        transition RegisterSession {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RegisterSession { session_id }
            update {
                self.session_id = Some(session_id);
            }
            to Idle
        }

        // 3. UnregisterSession: per-phase → Idle (NOT a self-loop, goes to Idle)
        // Cannot use per_phase because target is always Idle, not source phase.
        transition UnregisterSessionIdle {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Idle
        }
        transition UnregisterSessionAttached {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Idle
        }
        transition UnregisterSessionRunning {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Idle
        }
        transition UnregisterSessionRetired {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Idle
        }
        transition UnregisterSessionStopped {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Idle
        }

        // 4. ReconfigureSessionLlmIdentity: Attached + Running self-loops
        transition ReconfigureSessionLlmIdentityAttached {
            on input ReconfigureSessionLlmIdentity {
                previous_identity, previous_visibility_state,
                previous_capability_surface, previous_capability_surface_status,
                target_identity, target_capability_surface,
                next_visibility_state, next_capability_base_filter,
                next_active_visibility_revision, tool_visibility_delta
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {}
            to Attached
        }
        transition ReconfigureSessionLlmIdentityRunning {
            on input ReconfigureSessionLlmIdentity {
                previous_identity, previous_visibility_state,
                previous_capability_surface, previous_capability_surface_status,
                target_identity, target_capability_surface,
                next_visibility_state, next_capability_base_filter,
                next_active_visibility_revision, tool_visibility_delta
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {}
            to Running
        }

        // Phase 1 model-routing authority. These transitions own the scoped
        // override stack and image-operation phase projection; the shell only
        // realizes provider / persistence mechanics from the effects.
        transition SetModelRoutingBaseline {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SetModelRoutingBaseline { baseline_model, realtime_capable }
            guard "session_registered" { self.session_id != None }
            update {
                self.model_routing_baseline_model = Some(baseline_model);
                self.model_routing_baseline_realtime = Some(realtime_capable);
                self.model_routing_topology_epoch = self.model_routing_topology_epoch + 1;
            }
            to Idle
            emit ModelRoutingStatusChanged { topology_epoch: self.model_routing_topology_epoch }
        }

        transition RequestFiniteSwitchTurnApprovalUnavailable {
            per_phase [Idle, Attached, Running]
            on input RequestFiniteSwitchTurn {
                request_id, target_model, turns, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "baseline_known" { self.model_routing_baseline_model != None }
            guard "approval_unavailable" { requires_approval && !approval_available }
            update {
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::ApprovalRequiredButUnavailable);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::ApprovalRequiredButUnavailable }
        }

        transition RequestFiniteSwitchTurnApprovalDenied {
            per_phase [Idle, Attached, Running]
            on input RequestFiniteSwitchTurn {
                request_id, target_model, turns, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "approval_denied" { requires_approval && approval_available && approval_denied }
            update {
                self.model_routing_approval_phases.insert(request_id, RoutingApprovalPhase::Denied);
                self.model_routing_approval_parent_kind.insert(request_id, RoutingApprovalParentKind::SwitchTurn);
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::DeniedDuringApproval);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::DeniedDuringApproval }
            emit ModelRoutingApprovalTerminalized { approval_id: request_id, phase: RoutingApprovalPhase::Denied }
        }

        transition RequestFiniteSwitchTurnRealtimeConflict {
            per_phase [Idle, Attached, Running]
            on input RequestFiniteSwitchTurn {
                request_id, target_model, turns, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "realtime_conflict" {
                self.realtime_intent_present && !target_realtime_capable && !realtime_detach_allowed
            }
            update {
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::RealtimeTransportConflict);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::RealtimeTransportConflict }
        }

        transition RequestFiniteSwitchTurnScopedConflict {
            per_phase [Idle, Attached, Running]
            on input RequestFiniteSwitchTurn {
                request_id, target_model, turns, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "scoped_conflict" {
                self.model_routing_turn_override_id != None
                || self.model_routing_operation_override_id != None
                || self.model_routing_pending_switch_request_id != None
            }
            update {
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::ScopedOverrideConflict);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::ScopedOverrideConflict }
        }

        transition RequestFiniteSwitchTurnAccepted {
            per_phase [Idle, Attached, Running]
            on input RequestFiniteSwitchTurn {
                request_id, target_model, turns, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "baseline_known" { self.model_routing_baseline_model != None }
            guard "positive_turns" { turns > 0 }
            guard "approval_satisfied" { !requires_approval || (approval_available && !approval_denied) }
            guard "no_realtime_conflict" {
                !self.realtime_intent_present || target_realtime_capable || realtime_detach_allowed
            }
            guard "no_scoped_conflict" {
                self.model_routing_turn_override_id == None
                && self.model_routing_operation_override_id == None
                && self.model_routing_pending_switch_request_id == None
            }
            update {
                if requires_approval {
                    self.model_routing_approval_phases.insert(request_id, RoutingApprovalPhase::Approved);
                    self.model_routing_approval_parent_kind.insert(request_id, RoutingApprovalParentKind::SwitchTurn);
                }
                self.model_routing_pending_switch_request_id = Some(request_id);
                self.model_routing_pending_switch_target_model = Some(target_model);
                self.model_routing_pending_switch_realtime = Some(target_realtime_capable);
                self.model_routing_pending_switch_turns = Some(turns);
                self.model_routing_pending_switch_phase = Some(RoutingSwitchTurnPhase::PendingForBoundary);
            }
            to Idle
        }

        transition RequestUntilChangedSwitchTurnAccepted {
            per_phase [Idle, Attached, Running]
            on input RequestUntilChangedSwitchTurn {
                request_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "baseline_known" { self.model_routing_baseline_model != None }
            guard "approval_satisfied" { !requires_approval || (approval_available && !approval_denied) }
            guard "no_realtime_conflict" {
                !self.realtime_intent_present || target_realtime_capable || realtime_detach_allowed
            }
            update {
                if requires_approval {
                    self.model_routing_approval_phases.insert(request_id, RoutingApprovalPhase::Approved);
                    self.model_routing_approval_parent_kind.insert(request_id, RoutingApprovalParentKind::SwitchTurn);
                }
                self.model_routing_pending_switch_request_id = Some(request_id);
                self.model_routing_pending_switch_target_model = Some(target_model);
                self.model_routing_pending_switch_realtime = Some(target_realtime_capable);
                self.model_routing_pending_switch_turns = None;
                self.model_routing_pending_switch_phase = Some(RoutingSwitchTurnPhase::ApplyingPersistentReconfigure);
            }
            to Idle
            emit SwitchTurnPersistentReconfigureRequested { request_id: request_id, target_model: target_model }
        }

        transition RequestUntilChangedSwitchTurnRealtimeConflict {
            per_phase [Idle, Attached, Running]
            on input RequestUntilChangedSwitchTurn {
                request_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "realtime_conflict" {
                self.realtime_intent_present && !target_realtime_capable && !realtime_detach_allowed
            }
            update {
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::RealtimeTransportConflict);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::RealtimeTransportConflict }
        }

        transition RequestUntilChangedSwitchTurnApprovalUnavailable {
            per_phase [Idle, Attached, Running]
            on input RequestUntilChangedSwitchTurn {
                request_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "baseline_known" { self.model_routing_baseline_model != None }
            guard "approval_unavailable" { requires_approval && !approval_available }
            update {
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::ApprovalRequiredButUnavailable);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::ApprovalRequiredButUnavailable }
        }

        transition RequestUntilChangedSwitchTurnApprovalDenied {
            per_phase [Idle, Attached, Running]
            on input RequestUntilChangedSwitchTurn {
                request_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed
            }
            guard "approval_denied" { requires_approval && approval_available && approval_denied }
            update {
                self.model_routing_approval_phases.insert(request_id, RoutingApprovalPhase::Denied);
                self.model_routing_approval_parent_kind.insert(request_id, RoutingApprovalParentKind::SwitchTurn);
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::Denied);
                self.model_routing_switch_denials.insert(request_id, RoutingDenialReason::DeniedDuringApproval);
            }
            to Idle
            emit SwitchTurnDenied { request_id: request_id, reason: RoutingDenialReason::DeniedDuringApproval }
            emit ModelRoutingApprovalTerminalized { approval_id: request_id, phase: RoutingApprovalPhase::Denied }
        }

        transition CompleteUntilChangedSwitchTurnReconfigure {
            per_phase [Idle, Attached, Running]
            on input CompleteUntilChangedSwitchTurnReconfigure { request_id }
            guard "pending_until_changed_reconfigure" {
                self.model_routing_pending_switch_request_id == Some(request_id)
                && self.model_routing_pending_switch_turns == None
                && self.model_routing_pending_switch_phase == Some(RoutingSwitchTurnPhase::ApplyingPersistentReconfigure)
            }
            update {
                self.model_routing_switch_terminal.insert(request_id, RoutingSwitchTurnTerminal::PersistentReconfigureApplied);
                self.model_routing_pending_switch_request_id = None;
                self.model_routing_pending_switch_target_model = None;
                self.model_routing_pending_switch_realtime = None;
                self.model_routing_pending_switch_phase = None;
            }
            to Idle
            emit ModelRoutingStatusChanged { topology_epoch: self.model_routing_topology_epoch }
        }

        transition AdmitPendingFiniteSwitchTurn {
            per_phase [Idle, Attached, Running]
            on input AdmitModelRoutingAssistantTurn
            guard "pending_finite" {
                self.model_routing_pending_switch_request_id != None
                && self.model_routing_pending_switch_turns != None
            }
            update {
                self.model_routing_turn_override_id = Some(self.model_routing_pending_switch_request_id.get("value"));
                self.model_routing_turn_request_id = Some(self.model_routing_pending_switch_request_id.get("value"));
                self.model_routing_turn_target_model = Some(self.model_routing_pending_switch_target_model.get("value"));
                self.model_routing_turn_realtime = Some(self.model_routing_pending_switch_realtime.get("value"));
                self.model_routing_turn_remaining_turns = Some(self.model_routing_pending_switch_turns.get("value"));
                self.model_routing_pending_switch_request_id = None;
                self.model_routing_pending_switch_target_model = None;
                self.model_routing_pending_switch_realtime = None;
                self.model_routing_pending_switch_turns = None;
                self.model_routing_pending_switch_phase = None;
                self.model_routing_topology_epoch = self.model_routing_topology_epoch + 1;
            }
            to Idle
            emit SwitchTurnFiniteOverrideActivated {
                request_id: self.model_routing_turn_request_id.get("value"),
                target_model: self.model_routing_turn_target_model.get("value"),
                turns_remaining: self.model_routing_turn_remaining_turns.get("value")
            }
            emit ModelRoutingStatusChanged { topology_epoch: self.model_routing_topology_epoch }
        }

        transition DecrementFiniteSwitchTurn {
            per_phase [Idle, Attached, Running]
            on input AdmitModelRoutingAssistantTurn
            guard "active_multi_turn" {
                self.model_routing_turn_override_id != None
                && self.model_routing_turn_remaining_turns.get("value") > 1
            }
            update {
                self.model_routing_turn_remaining_turns = Some(self.model_routing_turn_remaining_turns.get("value") - 1);
            }
            to Idle
        }

        transition RestoreConsumedFiniteSwitchTurn {
            per_phase [Idle, Attached, Running]
            on input AdmitModelRoutingAssistantTurn
            guard "active_one_turn_remaining" {
                self.model_routing_turn_override_id != None
                && self.model_routing_turn_remaining_turns.get("value") == 1
            }
            update {
                self.model_routing_switch_terminal.insert(self.model_routing_turn_request_id.get("value"), RoutingSwitchTurnTerminal::ConsumedAndRestored);
                self.model_routing_turn_override_id = None;
                self.model_routing_turn_request_id = None;
                self.model_routing_turn_target_model = None;
                self.model_routing_turn_realtime = None;
                self.model_routing_turn_remaining_turns = None;
                self.model_routing_topology_epoch = self.model_routing_topology_epoch + 1;
            }
            to Idle
            emit SwitchTurnFiniteOverrideRestored { request_id: self.model_routing_turn_request_id.get("value") }
            emit ModelRoutingStatusChanged { topology_epoch: self.model_routing_topology_epoch }
        }

        transition BeginImageOperationScopedConflict {
            per_phase [Idle, Attached, Running]
            on input BeginImageOperation {
                operation_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed, requires_scoped_override
            }
            guard "operation_in_operation_conflict" { self.model_routing_operation_override_id != None }
            update {
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::Terminal);
                self.model_routing_image_terminals.insert(operation_id, RoutingImageTerminal::Denied);
                self.model_routing_image_denials.insert(operation_id, RoutingDenialReason::ScopedOverrideConflict);
            }
            to Idle
            emit ImageOperationDenied { operation_id: operation_id, reason: RoutingDenialReason::ScopedOverrideConflict }
        }

        transition BeginImageOperationRealtimeConflict {
            per_phase [Idle, Attached, Running]
            on input BeginImageOperation {
                operation_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed, requires_scoped_override
            }
            guard "realtime_conflict" {
                self.realtime_intent_present && !target_realtime_capable && !realtime_detach_allowed
            }
            update {
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::Terminal);
                self.model_routing_image_terminals.insert(operation_id, RoutingImageTerminal::Denied);
                self.model_routing_image_denials.insert(operation_id, RoutingDenialReason::RealtimeTransportConflict);
            }
            to Idle
            emit ImageOperationDenied { operation_id: operation_id, reason: RoutingDenialReason::RealtimeTransportConflict }
        }

        transition BeginImageOperationApprovalUnavailable {
            per_phase [Idle, Attached, Running]
            on input BeginImageOperation {
                operation_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed, requires_scoped_override
            }
            guard "approval_unavailable" { requires_approval && !approval_available }
            update {
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::Terminal);
                self.model_routing_image_terminals.insert(operation_id, RoutingImageTerminal::Denied);
                self.model_routing_image_denials.insert(operation_id, RoutingDenialReason::ApprovalRequiredButUnavailable);
            }
            to Idle
            emit ImageOperationDenied { operation_id: operation_id, reason: RoutingDenialReason::ApprovalRequiredButUnavailable }
        }

        transition BeginImageOperationApprovalDenied {
            per_phase [Idle, Attached, Running]
            on input BeginImageOperation {
                operation_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed, requires_scoped_override
            }
            guard "approval_denied" { requires_approval && approval_available && approval_denied }
            update {
                self.model_routing_approval_phases.insert(operation_id, RoutingApprovalPhase::Denied);
                self.model_routing_approval_parent_kind.insert(operation_id, RoutingApprovalParentKind::ImageOperation);
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::Terminal);
                self.model_routing_image_terminals.insert(operation_id, RoutingImageTerminal::Denied);
                self.model_routing_image_denials.insert(operation_id, RoutingDenialReason::DeniedDuringApproval);
            }
            to Idle
            emit ImageOperationDenied { operation_id: operation_id, reason: RoutingDenialReason::DeniedDuringApproval }
            emit ModelRoutingApprovalTerminalized { approval_id: operation_id, phase: RoutingApprovalPhase::Denied }
        }

        transition BeginImageOperationAccepted {
            per_phase [Idle, Attached, Running]
            on input BeginImageOperation {
                operation_id, target_model, target_realtime_capable,
                requires_approval, approval_available, approval_denied,
                realtime_detach_allowed, requires_scoped_override
            }
            guard "baseline_known" { self.model_routing_baseline_model != None }
            guard "no_operation_in_operation" { self.model_routing_operation_override_id == None }
            guard "approval_satisfied" { !requires_approval || (approval_available && !approval_denied) }
            guard "no_realtime_conflict" {
                !self.realtime_intent_present || target_realtime_capable || realtime_detach_allowed
            }
            update {
                if requires_approval {
                    self.model_routing_approval_phases.insert(operation_id, RoutingApprovalPhase::Approved);
                    self.model_routing_approval_parent_kind.insert(operation_id, RoutingApprovalParentKind::ImageOperation);
                }
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::PlanResolved);
                self.model_routing_image_operation_target_models.insert(operation_id, target_model);
                self.model_routing_image_operation_realtime.insert(operation_id, target_realtime_capable);
                if requires_scoped_override {
                    self.model_routing_image_operation_requires_scoped_override.insert(operation_id, true);
                }
            }
            to Idle
            emit ImageOperationPhaseChanged { operation_id: operation_id, phase: RoutingImageOperationPhase::PlanResolved }
        }

        transition ActivateImageOperationOverride {
            per_phase [Idle, Attached, Running]
            on input ActivateImageOperationOverride { operation_id, target_model, target_realtime_capable }
            guard "operation_plan_resolved" { self.model_routing_image_operation_target_models.contains_key(operation_id) }
            guard "operation_requires_scoped_override" {
                self.model_routing_image_operation_requires_scoped_override.contains_key(operation_id)
            }
            guard "no_operation_override_active" { self.model_routing_operation_override_id == None }
            update {
                self.model_routing_operation_override_id = Some(operation_id);
                self.model_routing_operation_target_model = Some(target_model);
                self.model_routing_operation_realtime = Some(target_realtime_capable);
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::ScopedOverrideActive);
                self.model_routing_topology_epoch = self.model_routing_topology_epoch + 1;
            }
            to Idle
            emit ImageOperationPhaseChanged { operation_id: operation_id, phase: RoutingImageOperationPhase::ScopedOverrideActive }
            emit ModelRoutingStatusChanged { topology_epoch: self.model_routing_topology_epoch }
        }

        transition CompleteImageOperation {
            per_phase [Idle, Attached, Running]
            on input CompleteImageOperation { operation_id, terminal, terminal_payload }
            guard "operation_active" { self.model_routing_operation_override_id == Some(operation_id) }
            guard "operation_requires_scoped_override" {
                self.model_routing_image_operation_requires_scoped_override.contains_key(operation_id)
            }
            update {
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::RestoringScopedOverride);
                self.model_routing_image_terminals.insert(operation_id, terminal);
                self.model_routing_image_terminal_payloads.insert(operation_id, terminal_payload);
            }
            to Idle
            emit ImageOperationPhaseChanged { operation_id: operation_id, phase: RoutingImageOperationPhase::RestoringScopedOverride }
        }

        transition CompleteImageOperationWithoutScopedOverride {
            per_phase [Idle, Attached, Running]
            on input CompleteImageOperation { operation_id, terminal, terminal_payload }
            guard "operation_plan_resolved" { self.model_routing_image_operation_target_models.contains_key(operation_id) }
            guard "operation_does_not_require_scoped_override" {
                !self.model_routing_image_operation_requires_scoped_override.contains_key(operation_id)
            }
            guard "no_operation_override_active" { self.model_routing_operation_override_id == None }
            update {
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::Terminal);
                self.model_routing_image_terminals.insert(operation_id, terminal);
                self.model_routing_image_terminal_payloads.insert(operation_id, terminal_payload);
                self.model_routing_image_operation_target_models.remove(operation_id);
                self.model_routing_image_operation_realtime.remove(operation_id);
                self.model_routing_image_operation_requires_scoped_override.remove(operation_id);
            }
            to Idle
            emit ImageOperationPhaseChanged { operation_id: operation_id, phase: RoutingImageOperationPhase::Terminal }
        }

        transition RestoreImageOperationOverride {
            per_phase [Idle, Attached, Running]
            on input RestoreImageOperationOverride { operation_id }
            guard "operation_active" { self.model_routing_operation_override_id == Some(operation_id) }
            update {
                self.model_routing_operation_override_id = None;
                self.model_routing_operation_target_model = None;
                self.model_routing_operation_realtime = None;
                self.model_routing_image_operation_phases.insert(operation_id, RoutingImageOperationPhase::Terminal);
                self.model_routing_image_operation_target_models.remove(operation_id);
                self.model_routing_image_operation_realtime.remove(operation_id);
                self.model_routing_image_operation_requires_scoped_override.remove(operation_id);
                self.model_routing_topology_epoch = self.model_routing_topology_epoch + 1;
            }
            to Idle
            emit ImageOperationPhaseChanged { operation_id: operation_id, phase: RoutingImageOperationPhase::Terminal }
            emit ModelRoutingStatusChanged { topology_epoch: self.model_routing_topology_epoch }
        }

        // 5. StagePersistentFilter: per-phase self-loop, guard session_registered
        transition StagePersistentFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StagePersistentFilter { filter, witnesses }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 6. RequestDeferredTools: per-phase self-loop, guard session_registered
        transition RequestDeferredTools {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequestDeferredTools { authorities }
            guard "session_registered" { self.session_id != None }
            guard "deferred_authorities_non_empty" { authorities != EmptyMap }
            guard "deferred_authorities_have_identity" {
                deferred_authorities_have_identity(authorities.keys(), authorities)
            }
            update {
                self.next_staged_visibility_revision = self.next_staged_visibility_revision + 1;
                self.staged_deferred_names = authorities.keys();
                self.staged_deferred_authorities = authorities;
                self.staged_visibility_revision = self.next_staged_visibility_revision;
            }
            to Idle
        }

        // 7. PrepareBindings: different source→target mappings per phase
        // Initializing → Initializing (no guard, emits RuntimeBound)
        transition PrepareBindingsInitializing {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Initializing }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Initializing
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Idle → Attached
        transition PrepareBindingsIdle {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Attached → Attached
        transition PrepareBindingsAttached {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Running → Running
        transition PrepareBindingsRunning {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Running
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Retired → Retired
        transition PrepareBindingsRetired {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Retired
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Stopped → Stopped (inline in hand-written catalog)
        transition PrepareBindingsStopped {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Stopped
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 8. SetPeerIngressContext: per-phase self-loop, guard session_registered
        transition SetPeerIngressContext {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SetPeerIngressContext { keep_alive }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 9. NotifyDrainExited: per-phase self-loop, guard session_registered, emit RuntimeNotice
        transition NotifyDrainExited {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input NotifyDrainExited { reason }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit RuntimeNotice { kind: RuntimeNoticeKind::Drain, detail: "drain exited" }
        }

        // 10. InterruptCurrentRun: Attached + Running self-loops
        transition InterruptCurrentRunAttached {
            on input InterruptCurrentRun
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit WakeInterrupt
            emit RequestCancellationAtBoundary
        }
        transition InterruptCurrentRun {
            on input InterruptCurrentRun
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit WakeInterrupt
            emit RequestCancellationAtBoundary
        }

        // 11. CancelAfterBoundary: Attached + Running self-loops
        transition CancelAfterBoundaryAttached {
            on input CancelAfterBoundary { reason }
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit RequestCancellationAtBoundary
            emit RuntimeEffectFact { kind: RuntimeEffectKind::CancelAfterBoundary, reason: reason }
        }
        transition CancelAfterBoundary {
            on input CancelAfterBoundary { reason }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit RequestCancellationAtBoundary
            emit RuntimeEffectFact { kind: RuntimeEffectKind::CancelAfterBoundary, reason: reason }
        }

        // 12. BoundaryAppliedPublish: Running self-loop (signal)
        transition BoundaryAppliedPublish {
            on signal BoundaryApplied { revision }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit CommittedVisibleSetPublished { revision: revision }
        }

        // 13. PublishCommittedVisibleSet: per-phase self-loop, complex guards
        transition PublishCommittedVisibleSetIdle {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_deferred_authorities, staged_deferred_authorities,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names
                    && active_deferred_authorities == staged_deferred_authorities)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            guard "active_deferred_authorities_cover_names" {
                for_all(requested_name in active_requested_deferred_names, active_deferred_authorities.contains_key(requested_name))
            }
            guard "staged_deferred_authorities_cover_names" {
                for_all(requested_name in staged_requested_deferred_names, staged_deferred_authorities.contains_key(requested_name))
            }
            guard "active_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in active_deferred_authorities.keys(), active_requested_deferred_names.contains(witnessed_name))
            }
            guard "staged_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in staged_deferred_authorities.keys(), staged_requested_deferred_names.contains(witnessed_name))
            }
            guard "active_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(active_requested_deferred_names, active_deferred_authorities)
            }
            guard "staged_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(staged_requested_deferred_names, staged_deferred_authorities)
            }
            update {
                self.active_filter = active_filter;
                self.staged_filter = staged_filter;
                self.active_deferred_names = active_requested_deferred_names;
                self.staged_deferred_names = staged_requested_deferred_names;
                self.active_deferred_authorities = active_deferred_authorities;
                self.staged_deferred_authorities = staged_deferred_authorities;
                self.active_visibility_revision = active_visibility_revision;
                self.staged_visibility_revision = staged_visibility_revision;
                // Sync the DSL monotonic counter to at least the published
                // active revision — guard `active_not_behind_staged` already
                // ensures active >= staged, so max == active here. Keeps the
                // counter honest when external callers install a visibility
                // state that advanced past the DSL's local history
                // (e.g. recovery, cross-session hot-swap).
                if active_visibility_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = active_visibility_revision;
                }
            }
            to Idle
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetAttached {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_deferred_authorities, staged_deferred_authorities,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names
                    && active_deferred_authorities == staged_deferred_authorities)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            guard "active_deferred_authorities_cover_names" {
                for_all(requested_name in active_requested_deferred_names, active_deferred_authorities.contains_key(requested_name))
            }
            guard "staged_deferred_authorities_cover_names" {
                for_all(requested_name in staged_requested_deferred_names, staged_deferred_authorities.contains_key(requested_name))
            }
            guard "active_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in active_deferred_authorities.keys(), active_requested_deferred_names.contains(witnessed_name))
            }
            guard "staged_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in staged_deferred_authorities.keys(), staged_requested_deferred_names.contains(witnessed_name))
            }
            guard "active_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(active_requested_deferred_names, active_deferred_authorities)
            }
            guard "staged_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(staged_requested_deferred_names, staged_deferred_authorities)
            }
            update {
                self.active_filter = active_filter;
                self.staged_filter = staged_filter;
                self.active_deferred_names = active_requested_deferred_names;
                self.staged_deferred_names = staged_requested_deferred_names;
                self.active_deferred_authorities = active_deferred_authorities;
                self.staged_deferred_authorities = staged_deferred_authorities;
                self.active_visibility_revision = active_visibility_revision;
                self.staged_visibility_revision = staged_visibility_revision;
                if active_visibility_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = active_visibility_revision;
                }
            }
            to Attached
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetRunning {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_deferred_authorities, staged_deferred_authorities,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names
                    && active_deferred_authorities == staged_deferred_authorities)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            guard "active_deferred_authorities_cover_names" {
                for_all(requested_name in active_requested_deferred_names, active_deferred_authorities.contains_key(requested_name))
            }
            guard "staged_deferred_authorities_cover_names" {
                for_all(requested_name in staged_requested_deferred_names, staged_deferred_authorities.contains_key(requested_name))
            }
            guard "active_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in active_deferred_authorities.keys(), active_requested_deferred_names.contains(witnessed_name))
            }
            guard "staged_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in staged_deferred_authorities.keys(), staged_requested_deferred_names.contains(witnessed_name))
            }
            guard "active_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(active_requested_deferred_names, active_deferred_authorities)
            }
            guard "staged_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(staged_requested_deferred_names, staged_deferred_authorities)
            }
            update {
                self.active_filter = active_filter;
                self.staged_filter = staged_filter;
                self.active_deferred_names = active_requested_deferred_names;
                self.staged_deferred_names = staged_requested_deferred_names;
                self.active_deferred_authorities = active_deferred_authorities;
                self.staged_deferred_authorities = staged_deferred_authorities;
                self.active_visibility_revision = active_visibility_revision;
                self.staged_visibility_revision = staged_visibility_revision;
                if active_visibility_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = active_visibility_revision;
                }
            }
            to Running
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetRetired {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_deferred_authorities, staged_deferred_authorities,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names
                    && active_deferred_authorities == staged_deferred_authorities)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            guard "active_deferred_authorities_cover_names" {
                for_all(requested_name in active_requested_deferred_names, active_deferred_authorities.contains_key(requested_name))
            }
            guard "staged_deferred_authorities_cover_names" {
                for_all(requested_name in staged_requested_deferred_names, staged_deferred_authorities.contains_key(requested_name))
            }
            guard "active_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in active_deferred_authorities.keys(), active_requested_deferred_names.contains(witnessed_name))
            }
            guard "staged_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in staged_deferred_authorities.keys(), staged_requested_deferred_names.contains(witnessed_name))
            }
            guard "active_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(active_requested_deferred_names, active_deferred_authorities)
            }
            guard "staged_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(staged_requested_deferred_names, staged_deferred_authorities)
            }
            update {
                self.active_filter = active_filter;
                self.staged_filter = staged_filter;
                self.active_deferred_names = active_requested_deferred_names;
                self.staged_deferred_names = staged_requested_deferred_names;
                self.active_deferred_authorities = active_deferred_authorities;
                self.staged_deferred_authorities = staged_deferred_authorities;
                self.active_visibility_revision = active_visibility_revision;
                self.staged_visibility_revision = staged_visibility_revision;
                if active_visibility_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = active_visibility_revision;
                }
            }
            to Retired
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetStopped {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_deferred_authorities, staged_deferred_authorities,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names
                    && active_deferred_authorities == staged_deferred_authorities)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            guard "active_deferred_authorities_cover_names" {
                for_all(requested_name in active_requested_deferred_names, active_deferred_authorities.contains_key(requested_name))
            }
            guard "staged_deferred_authorities_cover_names" {
                for_all(requested_name in staged_requested_deferred_names, staged_deferred_authorities.contains_key(requested_name))
            }
            guard "active_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in active_deferred_authorities.keys(), active_requested_deferred_names.contains(witnessed_name))
            }
            guard "staged_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in staged_deferred_authorities.keys(), staged_requested_deferred_names.contains(witnessed_name))
            }
            guard "active_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(active_requested_deferred_names, active_deferred_authorities)
            }
            guard "staged_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(staged_requested_deferred_names, staged_deferred_authorities)
            }
            update {
                self.active_filter = active_filter;
                self.staged_filter = staged_filter;
                self.active_deferred_names = active_requested_deferred_names;
                self.staged_deferred_names = staged_requested_deferred_names;
                self.active_deferred_authorities = active_deferred_authorities;
                self.staged_deferred_authorities = staged_deferred_authorities;
                self.active_visibility_revision = active_visibility_revision;
                self.staged_visibility_revision = staged_visibility_revision;
                if active_visibility_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = active_visibility_revision;
                }
            }
            to Stopped
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }

        // 14. Retire: from [Idle, Attached, Running] → Retired
        transition RetireRequestedFromIdle {
            on input Retire { session_id }
            guard {
                self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Running
            }
            update {}
            to Retired
            emit RuntimeRetired { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        transition RetireAlreadyRetired {
            on input Retire { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {}
            to Retired
        }

        // 15. Reset: from [Initializing, Idle, Attached, Retired] → Idle
        transition Reset {
            on input Reset
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Retired
            }
            update {
                self.current_run_id = None;
                self.active_fence_token = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Idle
            emit RuntimeNotice { kind: RuntimeNoticeKind::Reset, detail: "runtime reset" }
        }

        // 16. StopRuntimeExecutor: different behavior per phase
        // Unbound (Initializing, Idle, Retired) → Stopped
        transition StopRuntimeExecutorUnbound {
            on input StopRuntimeExecutor { reason }
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Retired
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Stop, detail: "runtime executor stopped" }
            emit RuntimeEffectFact { kind: RuntimeEffectKind::StopRuntimeExecutor, reason: reason }
        }
        // Attached → Attached (self-loop)
        transition StopRuntimeExecutorAttached {
            on input StopRuntimeExecutor { reason }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Attached
            emit RuntimeNotice { kind: RuntimeNoticeKind::Stop, detail: "runtime executor stopped" }
            emit RuntimeEffectFact { kind: RuntimeEffectKind::StopRuntimeExecutor, reason: reason }
        }
        // Running → Running (self-loop)
        transition StopRuntimeExecutorRunning {
            on input StopRuntimeExecutor { reason }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Running
            emit RuntimeNotice { kind: RuntimeNoticeKind::Stop, detail: "runtime executor stopped" }
            emit RuntimeEffectFact { kind: RuntimeEffectKind::StopRuntimeExecutor, reason: reason }
        }

        // RuntimeExecutorExited: async finalization of the stop-runtime path.
        // Fired by the runtime loop after apply_executor_control sees the
        // StopRuntimeExecutor control command complete and the driver has
        // flipped to Stopped. Moves the DSL phase from whatever it was at
        // stop-request time to Stopped.
        transition RuntimeExecutorExitedFromAttached {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Exit, detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromRunning {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Exit, detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromIdle {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Exit, detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromRetired {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Exit, detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromStopped {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }

        // 17. Destroy: from all non-Destroyed bound runtime phases → Destroyed.
        transition DestroyInitializing {
            on input Destroy { session_id }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Destroyed
            emit RuntimeDestroyed { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        transition Destroy {
            on input Destroy { session_id }
            guard {
                self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Retired
                || self.lifecycle_phase == Phase::Stopped
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
                self.registration_phase = RegistrationPhase::Queuing;
            }
            to Destroyed
            emit RuntimeDestroyed { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 18. Recover: preserve coarse phase while replaying runtime state.
        transition RecoverInitializing {
            on input Recover
            guard { self.lifecycle_phase == Phase::Initializing }
            update {}
            to Initializing
            emit RuntimeNotice { kind: RuntimeNoticeKind::Recover, detail: "runtime recovered" }
        }
        transition RecoverIdle {
            on input Recover
            guard { self.lifecycle_phase == Phase::Idle }
            update {}
            to Idle
            emit RuntimeNotice { kind: RuntimeNoticeKind::Recover, detail: "runtime recovered" }
        }
        transition RecoverAttached {
            on input Recover
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit RuntimeNotice { kind: RuntimeNoticeKind::Recover, detail: "runtime recovered" }
        }
        transition RecoverRetired {
            on input Recover
            guard { self.lifecycle_phase == Phase::Retired }
            update {}
            to Retired
            emit RuntimeNotice { kind: RuntimeNoticeKind::Recover, detail: "runtime recovered" }
        }
        transition RecoverStopped {
            on input Recover
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit RuntimeNotice { kind: RuntimeNoticeKind::Recover, detail: "runtime recovered" }
        }

        // =====================================================================
        // Absorbed transitions
        // =====================================================================

        // 19. EnsureSessionWithExecutor
        // Idle → Attached (phase change), sets registration to Active
        transition EnsureSessionWithExecutorIdle {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.registration_phase = RegistrationPhase::Active;
            }
            to Attached
        }
        // Attached, Running: self-loop (already Active)
        transition EnsureSessionWithExecutorAttached {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.registration_phase = RegistrationPhase::Active;
            }
            to Attached
        }
        transition EnsureSessionWithExecutorRunning {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.registration_phase = RegistrationPhase::Active;
            }
            to Running
        }
        // Retired, Stopped: self-loop
        transition EnsureSessionWithExecutorRetired {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {}
            to Retired
        }
        transition EnsureSessionWithExecutorStopped {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }

        // 19. SetSilentIntents: per-phase, guard session_registered
        // Idle, Attached, Running, Retired: update intents
        transition SetSilentIntentsIdle {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Idle
        }
        transition SetSilentIntentsAttached {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Attached
        }
        transition SetSilentIntentsRunning {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Running
        }
        transition SetSilentIntentsRetired {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Retired
        }
        // Stopped: no-op (no update)
        transition SetSilentIntentsStopped {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_registered" { self.session_id != None }
            update {}
            to Stopped
        }

        // 20. Abort: per-phase self-loop, guard session_registered
        transition Abort {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input Abort { session_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 20b. Wait: per-phase self-loop, guard session_registered
        transition Wait {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input Wait { session_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 21. AbortAll: per-phase self-loop, no guard
        transition AbortAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortAll
            update {}
            to Idle
        }

        // 22. EnsureDrainRunning: Attached/Running self-loops, emit SpawnDrainTask
        transition EnsureDrainRunningAttached {
            on signal EnsureDrainRunning
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SpawnDrainTask
        }
        transition EnsureDrainRunningRunning {
            on signal EnsureDrainRunning
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit SpawnDrainTask
        }

        // 23. Ingest: Idle/Attached/Running self-loops, emit ResolveAdmission
        transition Ingest {
            per_phase [Idle, Attached, Running]
            on input Ingest { runtime_id, work_id, origin }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit ResolveAdmission
        }

        // 24. PublishEvent: per-phase self-loop, emit IngressNotice
        transition PublishEvent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PublishEvent { kind }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit IngressNotice
        }

        // 25. AcceptWithCompletion: complex, multiple variants per phase.
        //
        // The `wake_if_idle` flag is the machine-owned truth for "this
        // input must wake the runtime loop when it reaches idle" (e.g.
        // peer_response_terminal queued while the session is running).
        // Idle/Attached queued arms already emit WakeLoop unconditionally
        // — the caller is about to wake — so wake_if_idle is ignored in
        // those guards. The Running+Queued path splits on it.
        //
        // Idle + queued (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionIdleQueued {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Idle
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::WakeLoop }
        }
        // Idle + immediate (immediate=true, interrupt_yielding=false)
        transition AcceptWithCompletionIdleImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Idle
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::RequestImmediateProcessing }
        }
        // Attached + immediate — admission-only self-loop.
        //
        // Post-#32 W6-J (dogma #1 split): admission no longer transitions to
        // Running. The `Prepare` DSL input is the sole authority for
        // lifecycle_phase + current_run_id run-start; the runtime loop fires
        // it from `machine_begin_run` with the loop's fresh run_id.
        // AcceptWithCompletion*Immediate now only signals intent (post-
        // admission signal + SubmitRunPrimitive emit) so the loop wakes;
        // the DSL run-start happens when the loop calls Prepare.
        transition AcceptWithCompletionAttachedImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Attached
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::RequestImmediateProcessing }
            emit SubmitRunPrimitive
        }
        // Attached + queued (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionAttachedQueued {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Attached
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::WakeLoop }
        }
        // Running + queued passive (immediate=false, interrupt_yielding=false, wake_if_idle=false)
        transition AcceptWithCompletionRunningQueuedPassive {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            guard "wake_if_idle" { wake_if_idle == false }
            update {}
            to Running
            emit IngressAccepted
        }
        // Running + queued wake-if-idle (immediate=false, interrupt_yielding=false, wake_if_idle=true)
        //
        // The input is staged for the next run boundary *and* the machine
        // records a pending `WakeLoop` so the runtime loop observes the
        // wake on its first idle re-check. Drives the turn-driven
        // realtime async-peer-response path: an operator that called
        // send_request and is waiting must not strand the response on
        // durable context alone — the admission signal becomes the
        // authority that schedules the next turn.
        transition AcceptWithCompletionRunningQueuedWakeIfIdle {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            guard "wake_if_idle" { wake_if_idle == true }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::WakeLoop }
        }
        // Running + interrupt_yielding (immediate=false, interrupt_yielding=true)
        transition AcceptWithCompletionRunningInterruptYielding {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == true }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::InterruptYielding }
            emit RuntimeEffectFact { kind: RuntimeEffectKind::CancelAfterBoundary, reason: "peer admission requested cooperative boundary cancel" }
        }
        // Running + immediate (immediate=true, interrupt_yielding=false)
        transition AcceptWithCompletionRunningImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, wake_if_idle }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: PostAdmissionSignalKind::RequestImmediateProcessing }
            emit RuntimeEffectFact { kind: RuntimeEffectKind::CancelAfterBoundary, reason: "peer admission requested cooperative boundary cancel" }
        }

        // 26. AcceptWithoutWake: Idle/Attached/Running self-loops
        transition AcceptWithoutWake {
            per_phase [Idle, Attached, Running]
            on input AcceptWithoutWake { input_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit IngressAccepted
        }

        // 27. Peer-ingress classification: Attached/Running self-loops.
        //
        // Comms supplies only parsed transport facts. The DSL owns the
        // semantic classification result emitted on `PeerIngressClassified`:
        // peer input class, auth exemption, lifecycle subject, silent routing,
        // and response terminality. The legacy `EnqueueClassifiedEntry` effect
        // remains as the coarse queue signal for existing machine audits.
        transition ClassifyExternalEnvelopeMessageAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_message" { envelope_kind == PeerIngressEnvelopeClass::Message }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ActionableMessage,
                kind: PeerIngressAdmittedKind::Message,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeMessageRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_message" { envelope_kind == PeerIngressEnvelopeClass::Message }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ActionableMessage,
                kind: PeerIngressAdmittedKind::Message,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestPeerAddedAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_request_peer_added" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "mob.peer_added"
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleAdded,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerAdded),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestPeerAddedRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_request_peer_added" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "mob.peer_added"
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleAdded,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerAdded),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestPeerRetiredAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_request_peer_retired" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "mob.peer_retired"
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleRetired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerRetired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestPeerRetiredRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_request_peer_retired" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "mob.peer_retired"
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleRetired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerRetired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestPeerUnwiredAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_request_peer_unwired" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "mob.peer_unwired"
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleUnwired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerUnwired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestPeerUnwiredRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_request_peer_unwired" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "mob.peer_unwired"
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleUnwired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerUnwired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestSupervisorSilentAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_supervisor_silent_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "supervisor.bridge"
                && self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::SilentRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::SupervisorBridgeExempt,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestSupervisorSilentRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_supervisor_silent_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "supervisor.bridge"
                && self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::SilentRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::SupervisorBridgeExempt,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestSilentAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_silent_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent != "supervisor.bridge"
                && request_intent != "mob.peer_added"
                && request_intent != "mob.peer_retired"
                && request_intent != "mob.peer_unwired"
                && self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::SilentRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestSilentRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_silent_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent != "supervisor.bridge"
                && request_intent != "mob.peer_added"
                && request_intent != "mob.peer_retired"
                && request_intent != "mob.peer_unwired"
                && self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::SilentRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestSupervisorAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_supervisor_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "supervisor.bridge"
                && !self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ActionableRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::SupervisorBridgeExempt,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestSupervisorRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_supervisor_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent == "supervisor.bridge"
                && !self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ActionableRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::SupervisorBridgeExempt,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestActionableAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_actionable_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent != "supervisor.bridge"
                && request_intent != "mob.peer_added"
                && request_intent != "mob.peer_retired"
                && request_intent != "mob.peer_unwired"
                && !self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ActionableRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeRequestActionableRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_actionable_request" {
                envelope_kind == PeerIngressEnvelopeClass::Request
                && request_intent != "supervisor.bridge"
                && request_intent != "mob.peer_added"
                && request_intent != "mob.peer_retired"
                && request_intent != "mob.peer_unwired"
                && !self.silent_intent_overrides.contains(request_intent)
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ActionableRequest,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(item_id),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeLifecycleAddedAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_lifecycle_added" {
                envelope_kind == PeerIngressEnvelopeClass::Lifecycle
                && lifecycle_kind == PeerIngressLifecycleClass::PeerAdded
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleAdded,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerAdded),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeLifecycleAddedRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_lifecycle_added" {
                envelope_kind == PeerIngressEnvelopeClass::Lifecycle
                && lifecycle_kind == PeerIngressLifecycleClass::PeerAdded
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleAdded,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerAdded),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeLifecycleRetiredAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_lifecycle_retired" {
                envelope_kind == PeerIngressEnvelopeClass::Lifecycle
                && lifecycle_kind == PeerIngressLifecycleClass::PeerRetired
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleRetired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerRetired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeLifecycleRetiredRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_lifecycle_retired" {
                envelope_kind == PeerIngressEnvelopeClass::Lifecycle
                && lifecycle_kind == PeerIngressLifecycleClass::PeerRetired
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleRetired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerRetired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeLifecycleUnwiredAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_lifecycle_unwired" {
                envelope_kind == PeerIngressEnvelopeClass::Lifecycle
                && lifecycle_kind == PeerIngressLifecycleClass::PeerUnwired
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleUnwired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerUnwired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeLifecycleUnwiredRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_lifecycle_unwired" {
                envelope_kind == PeerIngressEnvelopeClass::Lifecycle
                && lifecycle_kind == PeerIngressLifecycleClass::PeerUnwired
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PeerLifecycleUnwired,
                kind: PeerIngressAdmittedKind::Request,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: Some(PeerIngressLifecycleClass::PeerUnwired),
                lifecycle_peer: Some(if lifecycle_peer_param.is_some()
                    && lifecycle_peer_param.get("value") != ""
                {
                    lifecycle_peer_param.get("value")
                } else {
                    from_peer
                }),
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeResponseAcceptedAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_response_accepted" {
                envelope_kind == PeerIngressEnvelopeClass::Response
                && response_status == PeerIngressResponseStatus::Accepted
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ResponseProgress,
                kind: PeerIngressAdmittedKind::Response,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: Some(PeerIngressResponseTerminality::Progress)
            }
        }
        transition ClassifyExternalEnvelopeResponseAcceptedRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_response_accepted" {
                envelope_kind == PeerIngressEnvelopeClass::Response
                && response_status == PeerIngressResponseStatus::Accepted
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ResponseProgress,
                kind: PeerIngressAdmittedKind::Response,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: Some(PeerIngressResponseTerminality::Progress)
            }
        }
        transition ClassifyExternalEnvelopeResponseCompletedAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_response_completed" {
                envelope_kind == PeerIngressEnvelopeClass::Response
                && response_status == PeerIngressResponseStatus::Completed
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ResponseTerminal,
                kind: PeerIngressAdmittedKind::Response,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: Some(PeerIngressResponseTerminality::TerminalCompleted)
            }
        }
        transition ClassifyExternalEnvelopeResponseCompletedRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_response_completed" {
                envelope_kind == PeerIngressEnvelopeClass::Response
                && response_status == PeerIngressResponseStatus::Completed
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ResponseTerminal,
                kind: PeerIngressAdmittedKind::Response,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: Some(PeerIngressResponseTerminality::TerminalCompleted)
            }
        }
        transition ClassifyExternalEnvelopeResponseFailedAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_response_failed" {
                envelope_kind == PeerIngressEnvelopeClass::Response
                && response_status == PeerIngressResponseStatus::Failed
            }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ResponseTerminal,
                kind: PeerIngressAdmittedKind::Response,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: Some(PeerIngressResponseTerminality::TerminalFailed)
            }
        }
        transition ClassifyExternalEnvelopeResponseFailedRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_response_failed" {
                envelope_kind == PeerIngressEnvelopeClass::Response
                && response_status == PeerIngressResponseStatus::Failed
            }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::ResponseTerminal,
                kind: PeerIngressAdmittedKind::Response,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: Some(PeerIngressResponseTerminality::TerminalFailed)
            }
        }
        transition ClassifyExternalEnvelopeAckAttached {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_ack" { envelope_kind == PeerIngressEnvelopeClass::Ack }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::Ack,
                kind: PeerIngressAdmittedKind::Ack,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: None
            }
        }
        transition ClassifyExternalEnvelopeAckRunning {
            on signal ClassifyExternalEnvelope {
                item_id, from_peer, envelope_kind, request_intent, lifecycle_kind,
                lifecycle_peer_param, response_status, in_reply_to
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "peer_ingress_ack" { envelope_kind == PeerIngressEnvelopeClass::Ack }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::Ack,
                kind: PeerIngressAdmittedKind::Ack,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: Some(in_reply_to),
                response_terminality: None
            }
        }
        transition ClassifyPlainEventAttached {
            on signal ClassifyPlainEvent { source_name }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PlainEvent,
                kind: PeerIngressAdmittedKind::PlainEvent,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: None,
                response_terminality: None
            }
        }
        transition ClassifyPlainEventRunning {
            on signal ClassifyPlainEvent { source_name }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EnqueueClassifiedEntry
            emit PeerIngressClassified {
                class: PeerIngressInputClass::PlainEvent,
                kind: PeerIngressAdmittedKind::PlainEvent,
                auth: PeerIngressAuthClass::Required,
                lifecycle_kind: None,
                lifecycle_peer: None,
                request_id: None,
                response_terminality: None
            }
        }

        // 28. Prepare: Idle→Running, Attached→Running
        transition PrepareIdle {
            on input Prepare { session_id, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Idle);
            }
            to Running
            emit SubmitRunPrimitive
        }
        transition PrepareAttached {
            on input Prepare { session_id, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
            }
            to Running
            emit SubmitRunPrimitive
        }

        // 29. DrainQueuedRun: Retired→Running (signal)
        transition DrainQueuedRunRetired {
            on signal DrainQueuedRun { run_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Retired);
            }
            to Running
            emit SubmitRunPrimitive
        }

        // 30. Turn execution absorption
        transition StartConversationRunInitializing {
            on input StartConversationRun { run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            guard "conversation_shape_matches_primitive" {
                primitive_kind == TurnPrimitiveKind::ConversationTurn
                && (admitted_content_shape == ContentShape::Conversation
                    || admitted_content_shape == ContentShape::ConversationAndContext
                    || admitted_content_shape == ContentShape::Context
                    || admitted_content_shape == ContentShape::Empty)
            }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(primitive_kind);
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = vision_enabled;
                self.image_tool_results_enabled = image_tool_results_enabled;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = max_extraction_retries;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartConversationRunAttached {
            on input StartConversationRun { run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            guard "conversation_shape_matches_primitive" {
                primitive_kind == TurnPrimitiveKind::ConversationTurn
                && (admitted_content_shape == ContentShape::Conversation
                    || admitted_content_shape == ContentShape::ConversationAndContext
                    || admitted_content_shape == ContentShape::Context
                    || admitted_content_shape == ContentShape::Empty)
            }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(primitive_kind);
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = vision_enabled;
                self.image_tool_results_enabled = image_tool_results_enabled;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = max_extraction_retries;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartConversationRunRunning {
            on input StartConversationRun { run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            guard "conversation_shape_matches_primitive" {
                primitive_kind == TurnPrimitiveKind::ConversationTurn
                && (admitted_content_shape == ContentShape::Conversation
                    || admitted_content_shape == ContentShape::ConversationAndContext
                    || admitted_content_shape == ContentShape::Context
                    || admitted_content_shape == ContentShape::Empty)
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(primitive_kind);
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = vision_enabled;
                self.image_tool_results_enabled = image_tool_results_enabled;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = max_extraction_retries;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }

        transition StartImmediateAppendInitializing {
            on input StartImmediateAppend { run_id }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(TurnPrimitiveKind::ImmediateAppend);
                self.admitted_content_shape = Some(ContentShape::ImmediateAppend);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateAppendAttached {
            on input StartImmediateAppend { run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(TurnPrimitiveKind::ImmediateAppend);
                self.admitted_content_shape = Some(ContentShape::ImmediateAppend);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateAppendRunning {
            on input StartImmediateAppend { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(TurnPrimitiveKind::ImmediateAppend);
                self.admitted_content_shape = Some(ContentShape::ImmediateAppend);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }

        transition StartImmediateContextInitializing {
            on input StartImmediateContext { run_id }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(TurnPrimitiveKind::ImmediateContextAppend);
                self.admitted_content_shape = Some(ContentShape::ImmediateContext);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateContextAttached {
            on input StartImmediateContext { run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some(PreRunPhase::Attached);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(TurnPrimitiveKind::ImmediateContextAppend);
                self.admitted_content_shape = Some(ContentShape::ImmediateContext);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateContextRunning {
            on input StartImmediateContext { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_resettable" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = TurnPhase::ApplyingPrimitive;
                self.primitive_kind = Some(TurnPrimitiveKind::ImmediateContextAppend);
                self.admitted_content_shape = Some(ContentShape::ImmediateContext);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.terminal_cause_kind = None;
                self.last_runtime_apply_failure_cause = None;
                self.last_runtime_apply_failure_message = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }

        transition PrimitiveAppliedConversation {
            on input PrimitiveApplied
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_applying_conversation" {
                self.turn_phase == TurnPhase::ApplyingPrimitive
                && self.primitive_kind == Some(TurnPrimitiveKind::ConversationTurn)
            }
            update {
                self.turn_phase = TurnPhase::CallingLlm;
            }
            to Running
            emit TurnCheckCompaction
        }

        transition PrimitiveAppliedImmediate {
            on input PrimitiveApplied
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_applying_immediate" {
                self.turn_phase == TurnPhase::ApplyingPrimitive
                && (self.primitive_kind == Some(TurnPrimitiveKind::ImmediateAppend)
                    || self.primitive_kind == Some(TurnPrimitiveKind::ImmediateContextAppend))
            }
            update {
                self.boundary_count = self.boundary_count + 1;
                self.turn_phase = TurnPhase::Completed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Completed);
            }
            to Running
            emit TurnBoundaryApplied { run_id: self.current_run_id.get("value"), boundary_sequence: self.boundary_count }
            emit TurnRunCompleted { run_id: self.current_run_id.get("value"), outcome: TurnTerminalOutcome::Completed }
            emit TurnCheckCompaction
        }

        transition LlmReturnedToolCallsPositive {
            on input LlmReturnedToolCalls { tool_count }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_calling_llm" { self.turn_phase == TurnPhase::CallingLlm }
            guard "tool_count_positive" { tool_count > 0 }
            update {
                self.turn_phase = TurnPhase::WaitingForOps;
                self.tool_calls_pending = tool_count;
            }
            to Running
        }

        transition LlmReturnedToolCallsZero {
            on input LlmReturnedToolCalls { tool_count }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_calling_llm" { self.turn_phase == TurnPhase::CallingLlm }
            guard "tool_count_zero" { tool_count == 0 }
            update {
                self.turn_phase = TurnPhase::DrainingBoundary;
                self.tool_calls_pending = 0;
            }
            to Running
        }

        transition LlmReturnedTerminal {
            on input LlmReturnedTerminal
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_calling_llm" { self.turn_phase == TurnPhase::CallingLlm }
            update {
                self.turn_phase = TurnPhase::DrainingBoundary;
            }
            to Running
        }

        transition RegisterPendingOps {
            on input RegisterPendingOps { op_refs, barrier_operation_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_or_calling" { self.turn_phase == TurnPhase::CallingLlm || self.turn_phase == TurnPhase::WaitingForOps }
            update {
                self.turn_phase = TurnPhase::WaitingForOps;
                self.pending_op_refs = op_refs;
                self.barrier_operation_ids = barrier_operation_ids;
                self.has_barrier_ops = self.barrier_operation_ids != EmptySet;
                self.barrier_satisfied = self.barrier_operation_ids == EmptySet;
                self.tool_calls_pending = 0;
            }
            to Running
        }

        transition ToolCallsResolvedToCalling {
            on input ToolCallsResolved
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_for_ops" { self.turn_phase == TurnPhase::WaitingForOps }
            guard "barrier_not_satisfied" { self.barrier_satisfied == false }
            update {
                self.turn_phase = TurnPhase::CallingLlm;
            }
            to Running
        }

        transition ToolCallsResolvedToBoundary {
            on input ToolCallsResolved
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_for_ops" { self.turn_phase == TurnPhase::WaitingForOps }
            guard "barrier_satisfied" { self.barrier_satisfied == true }
            update {
                self.turn_phase = TurnPhase::DrainingBoundary;
            }
            to Running
        }

        transition OpsBarrierSatisfied {
            on input OpsBarrierSatisfied { operation_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_for_ops" { self.turn_phase == TurnPhase::WaitingForOps }
            guard "matching_barrier_ids" { operation_ids == self.barrier_operation_ids }
            update {
                self.barrier_satisfied = true;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
            }
            to Running
        }

        transition BoundaryContinue {
            on input BoundaryContinue
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_draining_boundary" { self.turn_phase == TurnPhase::DrainingBoundary }
            update {
                self.boundary_count = self.boundary_count + 1;
                self.turn_phase = TurnPhase::CallingLlm;
            }
            to Running
            emit TurnBoundaryApplied { run_id: self.current_run_id.get("value"), boundary_sequence: self.boundary_count }
            emit TurnCheckCompaction
        }

        transition BoundaryComplete {
            on input BoundaryComplete
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_draining_boundary" { self.turn_phase == TurnPhase::DrainingBoundary }
            update {
                self.boundary_count = self.boundary_count + 1;
                self.turn_phase = TurnPhase::Completed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Completed);
            }
            to Running
            emit TurnBoundaryApplied { run_id: self.current_run_id.get("value"), boundary_sequence: self.boundary_count }
            emit TurnRunCompleted { run_id: self.current_run_id.get("value"), outcome: TurnTerminalOutcome::Completed }
            emit TurnCheckCompaction
        }

        transition EnterExtraction {
            on input EnterExtraction { max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_draining_boundary" { self.turn_phase == TurnPhase::DrainingBoundary }
            update {
                self.turn_phase = TurnPhase::Extracting;
                self.max_extraction_retries = max_extraction_retries;
            }
            to Running
        }

        transition ExtractionStart {
            on input ExtractionStart
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == TurnPhase::Extracting }
            update {
                self.turn_phase = TurnPhase::CallingLlm;
            }
            to Running
        }

        transition ExtractionValidationPassed {
            on input ExtractionValidationPassed
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == TurnPhase::Extracting }
            update {
                self.turn_phase = TurnPhase::Completed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Completed);
            }
            to Running
            emit TurnRunCompleted { run_id: self.current_run_id.get("value"), outcome: TurnTerminalOutcome::Completed }
            emit TurnCheckCompaction
        }

        transition ExtractionValidationFailedRetry {
            on input ExtractionValidationFailed { error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == TurnPhase::Extracting }
            guard "retries_remaining" { self.extraction_attempts < self.max_extraction_retries }
            update {
                self.extraction_attempts = self.extraction_attempts + 1;
                self.turn_phase = TurnPhase::CallingLlm;
            }
            to Running
            emit TurnCheckCompaction
        }

        transition ExtractionValidationFailedExhausted {
            on input ExtractionValidationFailed { error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == TurnPhase::Extracting }
            guard "retries_exhausted" { self.extraction_attempts >= self.max_extraction_retries }
            update {
                self.extraction_attempts = self.extraction_attempts + 1;
                self.turn_phase = TurnPhase::Failed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Failed);
                self.terminal_cause_kind = Some(TurnTerminalCauseKind::StructuredOutputValidationFailed);
            }
            to Running
            emit TurnRunFailed {
                run_id: self.current_run_id.get("value"),
                terminal_cause_kind: TurnTerminalCauseKind::StructuredOutputValidationFailed,
                error: "ExtractionExhausted"
            }
        }

        transition RecoverableFailure {
            on input RecoverableFailure {
                failure_kind,
                retry_attempt,
                max_retries,
                selected_delay_ms,
                error
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_non_terminal" {
                self.turn_phase == TurnPhase::CallingLlm
                || self.turn_phase == TurnPhase::WaitingForOps
                || self.turn_phase == TurnPhase::DrainingBoundary
                || self.turn_phase == TurnPhase::Extracting
            }
            guard "retry_attempt_present" { retry_attempt > 0 }
            update {
                self.turn_phase = TurnPhase::ErrorRecovery;
                self.llm_retry_attempt = retry_attempt;
                self.llm_retry_max_retries = max_retries;
                self.llm_retry_selected_delay_ms = selected_delay_ms;
                self.llm_retry_last_failure_kind = Some(failure_kind);
            }
            to Running
        }

        transition FatalFailure {
            on input FatalFailure { terminal_cause_kind, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != TurnPhase::Completed && self.turn_phase != TurnPhase::Failed && self.turn_phase != TurnPhase::Cancelled }
            guard "terminal_cause_known" { terminal_cause_kind != TurnTerminalCauseKind::Unknown }
            update {
                self.turn_phase = TurnPhase::Failed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Failed);
                self.terminal_cause_kind = Some(terminal_cause_kind);
            }
            to Running
            emit TurnRunFailed {
                run_id: self.current_run_id.get("value"),
                terminal_cause_kind: terminal_cause_kind,
                error: error
            }
        }

        transition RetryRequested {
            on input RetryRequested { retry_attempt }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_error_recovery" { self.turn_phase == TurnPhase::ErrorRecovery }
            guard "retry_attempt_matches" { retry_attempt == self.llm_retry_attempt }
            update {
                self.turn_phase = TurnPhase::CallingLlm;
            }
            to Running
            emit TurnCheckCompaction
        }

        transition CancelNow {
            on input CancelNow
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_cancellable" {
                self.turn_phase != TurnPhase::Ready
                && self.turn_phase != TurnPhase::Completed
                && self.turn_phase != TurnPhase::Failed
                && self.turn_phase != TurnPhase::Cancelled
            }
            update {
                self.turn_phase = TurnPhase::Cancelling;
            }
            to Running
        }

        transition RequestCancelAfterBoundary {
            on input RequestCancelAfterBoundary
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_cancellable" {
                self.turn_phase != TurnPhase::Ready
                && self.turn_phase != TurnPhase::Completed
                && self.turn_phase != TurnPhase::Failed
                && self.turn_phase != TurnPhase::Cancelled
            }
            update {
                self.cancel_after_boundary = true;
            }
            to Running
        }

        transition CancellationObserved {
            on input CancellationObserved
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_cancelling" { self.turn_phase == TurnPhase::Cancelling }
            update {
                self.turn_phase = TurnPhase::Cancelled;
                self.terminal_outcome = Some(TurnTerminalOutcome::Cancelled);
            }
            to Running
            emit TurnRunCancelled { run_id: self.current_run_id.get("value"), reason: TurnCancellationReason::Observed }
        }

        transition AcknowledgeTerminal {
            on input AcknowledgeTerminal { outcome }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_terminal" {
                self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.turn_phase = TurnPhase::Ready;
                self.primitive_kind = None;
                self.admitted_content_shape = None;
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = Some(outcome);
                self.terminal_cause_kind = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
                self.llm_retry_attempt = 0;
                self.llm_retry_max_retries = 0;
                self.llm_retry_selected_delay_ms = 0;
                self.llm_retry_last_failure_kind = None;
            }
            to Running
        }

        transition TurnLimitReached {
            on input TurnLimitReached
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != TurnPhase::Completed && self.turn_phase != TurnPhase::Failed && self.turn_phase != TurnPhase::Cancelled }
            update {
                self.turn_phase = TurnPhase::Failed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Failed);
                self.terminal_cause_kind = Some(TurnTerminalCauseKind::TurnLimitReached);
            }
            to Running
            emit TurnRunFailed {
                run_id: self.current_run_id.get("value"),
                terminal_cause_kind: TurnTerminalCauseKind::TurnLimitReached,
                error: "TurnLimitReached"
            }
        }

        transition BudgetExhausted {
            on input BudgetExhausted
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != TurnPhase::Completed && self.turn_phase != TurnPhase::Failed && self.turn_phase != TurnPhase::Cancelled }
            update {
                self.turn_phase = TurnPhase::Failed;
                self.terminal_outcome = Some(TurnTerminalOutcome::BudgetExhausted);
                self.terminal_cause_kind = Some(TurnTerminalCauseKind::BudgetExhausted);
            }
            to Running
            emit TurnRunFailed {
                run_id: self.current_run_id.get("value"),
                terminal_cause_kind: TurnTerminalCauseKind::BudgetExhausted,
                error: "BudgetExhausted"
            }
        }

        transition TimeBudgetExceeded {
            on input TimeBudgetExceeded
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != TurnPhase::Completed && self.turn_phase != TurnPhase::Failed && self.turn_phase != TurnPhase::Cancelled }
            update {
                self.turn_phase = TurnPhase::Failed;
                self.terminal_outcome = Some(TurnTerminalOutcome::TimeBudgetExceeded);
                self.terminal_cause_kind = Some(TurnTerminalCauseKind::TimeBudgetExceeded);
            }
            to Running
            emit TurnRunFailed {
                run_id: self.current_run_id.get("value"),
                terminal_cause_kind: TurnTerminalCauseKind::TimeBudgetExceeded,
                error: "TimeBudgetExceeded"
            }
        }

        transition ForceCancelNoRun {
            on input ForceCancelNoRun
            guard { self.lifecycle_phase == Phase::Running }
            guard "no_run_bound" { self.current_run_id == None }
            guard "turn_ready" { self.turn_phase == TurnPhase::Ready }
            update {
                self.turn_phase = TurnPhase::Cancelled;
                self.terminal_outcome = Some(TurnTerminalOutcome::Cancelled);
            }
            to Running
        }

        transition RunCompleted {
            on input RunCompleted { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.turn_phase = TurnPhase::Completed;
                self.terminal_outcome = Some(TurnTerminalOutcome::Completed);
            }
            to Running
        }

        transition RunFailed {
            on input RunFailed { run_id, runtime_apply_failure_cause, runtime_apply_failure_message, terminal_outcome, terminal_cause_kind, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_matches_binding" { self.current_run_id == Some(run_id) }
            guard "terminal_cause_known" { terminal_cause_kind != TurnTerminalCauseKind::Unknown }
            update {
                self.turn_phase = TurnPhase::Failed;
                self.terminal_outcome = Some(terminal_outcome);
                self.terminal_cause_kind = Some(terminal_cause_kind);
                self.last_runtime_apply_failure_cause = runtime_apply_failure_cause;
                self.last_runtime_apply_failure_message = runtime_apply_failure_message;
            }
            to Running
        }

        transition RunCancelled {
            on input RunCancelled { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.turn_phase = TurnPhase::Cancelled;
                self.terminal_outcome = Some(TurnTerminalOutcome::Cancelled);
            }
            to Running
        }

        transition SurfaceRegisterAttached {
            on input SurfaceRegister { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.known_surfaces.insert(surface_id);
            }
            to Attached
        }
        transition SurfaceRegisterRunning {
            on input SurfaceRegister { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.known_surfaces.insert(surface_id);
            }
            to Running
        }

        transition SurfaceStageAddAttached {
            on input SurfaceStageAdd { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, SurfaceStagedOp::Add);
                self.reload_staged_surfaces.remove(surface_id);
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Attached
        }
        transition SurfaceStageAddRunning {
            on input SurfaceStageAdd { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, SurfaceStagedOp::Add);
                self.reload_staged_surfaces.remove(surface_id);
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Running
        }

        transition SurfaceStageRemoveAttached {
            on input SurfaceStageRemove { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, SurfaceStagedOp::Remove);
                self.reload_staged_surfaces.remove(surface_id);
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Attached
        }
        transition SurfaceStageRemoveRunning {
            on input SurfaceStageRemove { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, SurfaceStagedOp::Remove);
                self.reload_staged_surfaces.remove(surface_id);
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Running
        }

        transition SurfaceStageReloadAttached {
            on input SurfaceStageReload { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "surface_active" { self.active_surfaces.contains(surface_id) }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, SurfaceStagedOp::Reload);
                self.reload_staged_surfaces.insert(surface_id);
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Attached
        }
        transition SurfaceStageReloadRunning {
            on input SurfaceStageReload { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "surface_active" { self.active_surfaces.contains(surface_id) }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, SurfaceStagedOp::Reload);
                self.reload_staged_surfaces.insert(surface_id);
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Running
        }

        transition SurfaceApplyBoundaryAddAttached {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_add" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Add }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            guard "base_accepts_add" {
                !self.surface_base_state.contains_key(surface_id)
                || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Absent
                || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active
                || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removed
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.next_pending_task_sequence = self.next_pending_task_sequence + 1;
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::Add);
                self.surface_pending_task_sequence.insert(surface_id, self.next_pending_task_sequence);
                self.surface_pending_lineage_sequence.insert(surface_id, staged_intent_sequence);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Add);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Pending);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit ScheduleSurfaceCompletion {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Add,
                pending_task_sequence: self.next_pending_task_sequence,
                staged_intent_sequence: staged_intent_sequence,
                applied_at_turn: applied_at_turn,
            }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Add,
                phase: ExternalToolSurfaceDeltaPhase::Pending,
                cause: None,
            }
        }
        transition SurfaceApplyBoundaryAddRunning {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_add" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Add }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            guard "base_accepts_add" {
                !self.surface_base_state.contains_key(surface_id)
                || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Absent
                || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active
                || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removed
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.next_pending_task_sequence = self.next_pending_task_sequence + 1;
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::Add);
                self.surface_pending_task_sequence.insert(surface_id, self.next_pending_task_sequence);
                self.surface_pending_lineage_sequence.insert(surface_id, staged_intent_sequence);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Add);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Pending);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit ScheduleSurfaceCompletion {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Add,
                pending_task_sequence: self.next_pending_task_sequence,
                staged_intent_sequence: staged_intent_sequence,
                applied_at_turn: applied_at_turn,
            }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Add,
                phase: ExternalToolSurfaceDeltaPhase::Pending,
                cause: None,
            }
        }
        transition SurfaceApplyBoundaryReloadAttached {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_reload" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Reload }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "surface_active" { self.active_surfaces.contains(surface_id) }
            guard "base_active" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.next_pending_task_sequence = self.next_pending_task_sequence + 1;
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::Reload);
                self.surface_pending_task_sequence.insert(surface_id, self.next_pending_task_sequence);
                self.surface_pending_lineage_sequence.insert(surface_id, staged_intent_sequence);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Reload);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Pending);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit ScheduleSurfaceCompletion {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Reload,
                pending_task_sequence: self.next_pending_task_sequence,
                staged_intent_sequence: staged_intent_sequence,
                applied_at_turn: applied_at_turn,
            }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Reload,
                phase: ExternalToolSurfaceDeltaPhase::Pending,
                cause: None,
            }
        }
        transition SurfaceApplyBoundaryReloadRunning {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_reload" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Reload }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "surface_active" { self.active_surfaces.contains(surface_id) }
            guard "base_active" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.next_pending_task_sequence = self.next_pending_task_sequence + 1;
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::Reload);
                self.surface_pending_task_sequence.insert(surface_id, self.next_pending_task_sequence);
                self.surface_pending_lineage_sequence.insert(surface_id, staged_intent_sequence);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Reload);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Pending);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit ScheduleSurfaceCompletion {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Reload,
                pending_task_sequence: self.next_pending_task_sequence,
                staged_intent_sequence: staged_intent_sequence,
                applied_at_turn: applied_at_turn,
            }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Reload,
                phase: ExternalToolSurfaceDeltaPhase::Pending,
                cause: None,
            }
        }
        transition SurfaceApplyBoundaryRemoveDrainingAttached {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_remove" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Remove }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "base_active" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Removing);
                self.active_surfaces.remove(surface_id);
                self.visible_surfaces.remove(surface_id);
                self.surface_draining_since_ms.insert(surface_id, now_ms);
                self.surface_removal_timeout_at_ms.insert(surface_id, now_ms + self.removal_timeout_ms);
                self.surface_removal_applied_at_turn.insert(surface_id, applied_at_turn);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Remove);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Draining);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Remove,
                phase: ExternalToolSurfaceDeltaPhase::Draining,
                cause: None,
            }
        }
        transition SurfaceApplyBoundaryRemoveDrainingRunning {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_remove" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Remove }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "base_active" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Removing);
                self.active_surfaces.remove(surface_id);
                self.visible_surfaces.remove(surface_id);
                self.surface_draining_since_ms.insert(surface_id, now_ms);
                self.surface_removal_timeout_at_ms.insert(surface_id, now_ms + self.removal_timeout_ms);
                self.surface_removal_applied_at_turn.insert(surface_id, applied_at_turn);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Remove);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Draining);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Remove,
                phase: ExternalToolSurfaceDeltaPhase::Draining,
                cause: None,
            }
        }
        transition SurfaceApplyBoundaryRemoveNoopAttached {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_remove" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Remove }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "base_not_active" {
                !self.surface_base_state.contains_key(surface_id)
                || self.surface_base_state.get(surface_id).get("value") != ExternalToolSurfaceBaseState::Active
            }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
            }
            to Attached
        }
        transition SurfaceApplyBoundaryRemoveNoopRunning {
            on input SurfaceApplyBoundary { surface_id, now_ms, staged_intent_sequence, applied_at_turn }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == SurfacePhase::Operating }
            guard "staged_remove" { self.surface_staged_op.contains_key(surface_id) && self.surface_staged_op.get(surface_id).get("value") == SurfaceStagedOp::Remove }
            guard "staged_sequence_matches" { self.surface_staged_intent_sequence.contains_key(surface_id) && self.surface_staged_intent_sequence.get(surface_id).get("value") == staged_intent_sequence }
            guard "base_not_active" {
                !self.surface_base_state.contains_key(surface_id)
                || self.surface_base_state.get(surface_id).get("value") != ExternalToolSurfaceBaseState::Active
            }
            guard "no_pending_surface_op" {
                !self.surface_pending_op.contains_key(surface_id)
                || self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::None
            }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_staged_op.remove(surface_id);
                self.surface_staged_intent_sequence.remove(surface_id);
                self.reload_staged_surfaces.remove(surface_id);
            }
            to Running
        }

        transition SurfaceMarkPendingSucceededAddAttached {
            on input SurfaceMarkPendingSucceeded { surface_id, pending_task_sequence, staged_intent_sequence }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "pending_add" { self.surface_pending_op.contains_key(surface_id) && self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::Add }
            guard "pending_sequence_matches" { self.surface_pending_task_sequence.contains_key(surface_id) && self.surface_pending_task_sequence.get(surface_id).get("value") == pending_task_sequence }
            guard "pending_lineage_matches" { self.surface_pending_lineage_sequence.contains_key(surface_id) && self.surface_pending_lineage_sequence.get(surface_id).get("value") == staged_intent_sequence }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Active);
                self.active_surfaces.insert(surface_id);
                self.visible_surfaces.insert(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Add);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Applied);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Add,
                phase: ExternalToolSurfaceDeltaPhase::Applied,
                cause: None,
            }
        }
        transition SurfaceMarkPendingSucceededAddRunning {
            on input SurfaceMarkPendingSucceeded { surface_id, pending_task_sequence, staged_intent_sequence }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_add" { self.surface_pending_op.contains_key(surface_id) && self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::Add }
            guard "pending_sequence_matches" { self.surface_pending_task_sequence.contains_key(surface_id) && self.surface_pending_task_sequence.get(surface_id).get("value") == pending_task_sequence }
            guard "pending_lineage_matches" { self.surface_pending_lineage_sequence.contains_key(surface_id) && self.surface_pending_lineage_sequence.get(surface_id).get("value") == staged_intent_sequence }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Active);
                self.active_surfaces.insert(surface_id);
                self.visible_surfaces.insert(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Add);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Applied);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Add,
                phase: ExternalToolSurfaceDeltaPhase::Applied,
                cause: None,
            }
        }
        transition SurfaceMarkPendingSucceededReloadAttached {
            on input SurfaceMarkPendingSucceeded { surface_id, pending_task_sequence, staged_intent_sequence }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "pending_reload" { self.surface_pending_op.contains_key(surface_id) && self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::Reload }
            guard "pending_sequence_matches" { self.surface_pending_task_sequence.contains_key(surface_id) && self.surface_pending_task_sequence.get(surface_id).get("value") == pending_task_sequence }
            guard "pending_lineage_matches" { self.surface_pending_lineage_sequence.contains_key(surface_id) && self.surface_pending_lineage_sequence.get(surface_id).get("value") == staged_intent_sequence }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Active);
                self.active_surfaces.insert(surface_id);
                self.visible_surfaces.insert(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Reload);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Applied);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Reload,
                phase: ExternalToolSurfaceDeltaPhase::Applied,
                cause: None,
            }
        }
        transition SurfaceMarkPendingSucceededReloadRunning {
            on input SurfaceMarkPendingSucceeded { surface_id, pending_task_sequence, staged_intent_sequence }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_reload" { self.surface_pending_op.contains_key(surface_id) && self.surface_pending_op.get(surface_id).get("value") == SurfacePendingOp::Reload }
            guard "pending_sequence_matches" { self.surface_pending_task_sequence.contains_key(surface_id) && self.surface_pending_task_sequence.get(surface_id).get("value") == pending_task_sequence }
            guard "pending_lineage_matches" { self.surface_pending_lineage_sequence.contains_key(surface_id) && self.surface_pending_lineage_sequence.get(surface_id).get("value") == staged_intent_sequence }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Active);
                self.active_surfaces.insert(surface_id);
                self.visible_surfaces.insert(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Reload);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Applied);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Reload,
                phase: ExternalToolSurfaceDeltaPhase::Applied,
                cause: None,
            }
        }

        transition SurfaceMarkPendingFailedAttached {
            on input SurfaceMarkPendingFailed { surface_id, pending_task_sequence, staged_intent_sequence, cause }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "pending_sequence_matches" { self.surface_pending_task_sequence.contains_key(surface_id) && self.surface_pending_task_sequence.get(surface_id).get("value") == pending_task_sequence }
            guard "pending_lineage_matches" { self.surface_pending_lineage_sequence.contains_key(surface_id) && self.surface_pending_lineage_sequence.get(surface_id).get("value") == staged_intent_sequence }
            update {
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Failed);
            }
            to Attached
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: self.surface_last_delta_operation.get(surface_id).get("value"),
                phase: ExternalToolSurfaceDeltaPhase::Failed,
                cause: Some(cause),
            }
        }
        transition SurfaceMarkPendingFailedRunning {
            on input SurfaceMarkPendingFailed { surface_id, pending_task_sequence, staged_intent_sequence, cause }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_sequence_matches" { self.surface_pending_task_sequence.contains_key(surface_id) && self.surface_pending_task_sequence.get(surface_id).get("value") == pending_task_sequence }
            guard "pending_lineage_matches" { self.surface_pending_lineage_sequence.contains_key(surface_id) && self.surface_pending_lineage_sequence.get(surface_id).get("value") == staged_intent_sequence }
            update {
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Failed);
            }
            to Running
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: self.surface_last_delta_operation.get(surface_id).get("value"),
                phase: ExternalToolSurfaceDeltaPhase::Failed,
                cause: Some(cause),
            }
        }

        transition SurfaceCallStartedActiveAttached {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_active" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active }
            update {
                self.surface_inflight_calls.increment(surface_id, 1);
            }
            to Attached
        }
        transition SurfaceCallStartedActiveRunning {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_active" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active }
            update {
                self.surface_inflight_calls.increment(surface_id, 1);
            }
            to Running
        }
        transition SurfaceCallStartedRejectRemovingAttached {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_removing" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing }
            update {}
            to Attached
            emit RejectSurfaceCall { surface_id: surface_id, cause: ExternalToolSurfaceFailureCause::SurfaceDraining }
        }
        transition SurfaceCallStartedRejectRemovingRunning {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_removing" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing }
            update {}
            to Running
            emit RejectSurfaceCall { surface_id: surface_id, cause: ExternalToolSurfaceFailureCause::SurfaceDraining }
        }
        transition SurfaceCallStartedRejectUnavailableAttached {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_unavailable" {
                !self.surface_base_state.contains_key(surface_id)
                || (
                    self.surface_base_state.get(surface_id).get("value") != ExternalToolSurfaceBaseState::Active
                    && self.surface_base_state.get(surface_id).get("value") != ExternalToolSurfaceBaseState::Removing
                )
            }
            update {}
            to Attached
            emit RejectSurfaceCall { surface_id: surface_id, cause: ExternalToolSurfaceFailureCause::SurfaceUnavailable }
        }
        transition SurfaceCallStartedRejectUnavailableRunning {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_unavailable" {
                !self.surface_base_state.contains_key(surface_id)
                || (
                    self.surface_base_state.get(surface_id).get("value") != ExternalToolSurfaceBaseState::Active
                    && self.surface_base_state.get(surface_id).get("value") != ExternalToolSurfaceBaseState::Removing
                )
            }
            update {}
            to Running
            emit RejectSurfaceCall { surface_id: surface_id, cause: ExternalToolSurfaceFailureCause::SurfaceUnavailable }
        }

        transition SurfaceCallFinishedAttached {
            on input SurfaceCallFinished { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_active_or_removing" {
                self.surface_base_state.contains_key(surface_id)
                && (
                    self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active
                    || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing
                )
            }
            guard "inflight_calls_remain" { self.surface_inflight_calls.contains_key(surface_id) && self.surface_inflight_calls.get(surface_id).get("value") > 0 }
            update {
                self.surface_inflight_calls.insert(surface_id, self.surface_inflight_calls.get(surface_id).get("value") - 1);
            }
            to Attached
        }
        transition SurfaceCallFinishedRunning {
            on input SurfaceCallFinished { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_active_or_removing" {
                self.surface_base_state.contains_key(surface_id)
                && (
                    self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Active
                    || self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing
                )
            }
            guard "inflight_calls_remain" { self.surface_inflight_calls.contains_key(surface_id) && self.surface_inflight_calls.get(surface_id).get("value") > 0 }
            update {
                self.surface_inflight_calls.insert(surface_id, self.surface_inflight_calls.get(surface_id).get("value") - 1);
            }
            to Running
        }

        transition SurfaceFinalizeRemovalCleanAttached {
            on input SurfaceFinalizeRemovalClean { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_removing" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing }
            guard "no_inflight_calls_remain" {
                !self.surface_inflight_calls.contains_key(surface_id)
                || self.surface_inflight_calls.get(surface_id).get("value") == 0
            }
            update {
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Removed);
                self.active_surfaces.remove(surface_id);
                self.visible_surfaces.remove(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_draining_since_ms.remove(surface_id);
                self.surface_removal_timeout_at_ms.remove(surface_id);
                self.surface_removal_applied_at_turn.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Remove);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Applied);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit CloseSurfaceConnection { surface_id: surface_id }
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Remove,
                phase: ExternalToolSurfaceDeltaPhase::Applied,
                cause: None,
            }
        }
        transition SurfaceFinalizeRemovalCleanRunning {
            on input SurfaceFinalizeRemovalClean { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_removing" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing }
            guard "no_inflight_calls_remain" {
                !self.surface_inflight_calls.contains_key(surface_id)
                || self.surface_inflight_calls.get(surface_id).get("value") == 0
            }
            update {
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Removed);
                self.active_surfaces.remove(surface_id);
                self.visible_surfaces.remove(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_draining_since_ms.remove(surface_id);
                self.surface_removal_timeout_at_ms.remove(surface_id);
                self.surface_removal_applied_at_turn.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Remove);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Applied);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit CloseSurfaceConnection { surface_id: surface_id }
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Remove,
                phase: ExternalToolSurfaceDeltaPhase::Applied,
                cause: None,
            }
        }

        transition SurfaceFinalizeRemovalForcedAttached {
            on input SurfaceFinalizeRemovalForced { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_removing" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing }
            update {
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Removed);
                self.active_surfaces.remove(surface_id);
                self.visible_surfaces.remove(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_inflight_calls.insert(surface_id, 0);
                self.surface_draining_since_ms.remove(surface_id);
                self.surface_removal_timeout_at_ms.remove(surface_id);
                self.surface_removal_applied_at_turn.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Remove);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Forced);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
            emit CloseSurfaceConnection { surface_id: surface_id }
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Remove,
                phase: ExternalToolSurfaceDeltaPhase::Forced,
                cause: None,
            }
        }
        transition SurfaceFinalizeRemovalForcedRunning {
            on input SurfaceFinalizeRemovalForced { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_removing" { self.surface_base_state.contains_key(surface_id) && self.surface_base_state.get(surface_id).get("value") == ExternalToolSurfaceBaseState::Removing }
            update {
                self.surface_base_state.insert(surface_id, ExternalToolSurfaceBaseState::Removed);
                self.active_surfaces.remove(surface_id);
                self.visible_surfaces.remove(surface_id);
                self.surface_pending_op.insert(surface_id, SurfacePendingOp::None);
                self.surface_pending_task_sequence.insert(surface_id, 0);
                self.surface_pending_lineage_sequence.insert(surface_id, 0);
                self.surface_inflight_calls.insert(surface_id, 0);
                self.surface_draining_since_ms.remove(surface_id);
                self.surface_removal_timeout_at_ms.remove(surface_id);
                self.surface_removal_applied_at_turn.remove(surface_id);
                self.surface_last_delta_operation.insert(surface_id, ExternalToolSurfaceDeltaOperation::Remove);
                self.surface_last_delta_phase.insert(surface_id, ExternalToolSurfaceDeltaPhase::Forced);
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
            emit CloseSurfaceConnection { surface_id: surface_id }
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
            emit EmitExternalToolDelta {
                surface_id: surface_id,
                operation: ExternalToolSurfaceDeltaOperation::Remove,
                phase: ExternalToolSurfaceDeltaPhase::Forced,
                cause: None,
            }
        }

        transition SurfaceSnapshotAlignedAttached {
            on input SurfaceSnapshotAligned { epoch }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.snapshot_aligned_epoch = epoch;
            }
            to Attached
        }
        transition SurfaceSnapshotAlignedRunning {
            on input SurfaceSnapshotAligned { epoch }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.snapshot_aligned_epoch = epoch;
            }
            to Running
        }

        transition SurfaceShutdownAttached {
            on input SurfaceShutdown
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_phase = SurfacePhase::Shutdown;
            }
            to Attached
        }
        transition SurfaceShutdownRunning {
            on input SurfaceShutdown
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_phase = SurfacePhase::Shutdown;
            }
            to Running
        }

        // 31. Commit: Running → Idle/Attached/Retired (guard pre_run_phase + run_id match)
        transition CommitRunningToIdle {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some(PreRunPhase::Idle) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition CommitRunningToAttached {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some(PreRunPhase::Attached) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
        }
        transition CommitRunningToRetired {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some(PreRunPhase::Retired) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
        }

        // 32. Fail: Running → Idle/Attached/Retired (guard pre_run_phase + run_id match)
        transition FailRunningToIdle {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some(PreRunPhase::Idle) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            guard "turn_failed_with_cause" {
                self.turn_phase == TurnPhase::Failed
                && self.terminal_cause_kind != None
                && self.terminal_cause_kind != Some(TurnTerminalCauseKind::Unknown)
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
            emit RecordTerminalOutcome
        }
        transition FailRunningToAttached {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some(PreRunPhase::Attached) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            guard "turn_failed_with_cause" {
                self.turn_phase == TurnPhase::Failed
                && self.terminal_cause_kind != None
                && self.terminal_cause_kind != Some(TurnTerminalCauseKind::Unknown)
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
            emit RecordTerminalOutcome
        }
        transition FailRunningToRetired {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some(PreRunPhase::Retired) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            guard "turn_failed_with_cause" {
                self.turn_phase == TurnPhase::Failed
                && self.terminal_cause_kind != None
                && self.terminal_cause_kind != Some(TurnTerminalCauseKind::Unknown)
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
            emit RecordTerminalOutcome
        }

        // 32b. RollbackRun: Running -> Idle/Attached/Retired for non-terminal
        // runtime cleanup before a turn-level terminal failure exists. This is
        // not a semantic terminal-failure path and intentionally emits no
        // terminal outcome effect.
        transition RollbackRunRunningToIdle {
            on input RollbackRun { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some(PreRunPhase::Idle) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition RollbackRunRunningToAttached {
            on input RollbackRun { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some(PreRunPhase::Attached) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
        }
        transition RollbackRunRunningToRetired {
            on input RollbackRun { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some(PreRunPhase::Retired) }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
        }

        // 34. Recycle: from Idle/Retired → Idle, from Attached → Attached
        transition RecycleFromIdleOrRetired {
            on input Recycle
            guard {
                self.lifecycle_phase == Phase::Idle || self.lifecycle_phase == Phase::Retired
            }
            guard "session_registered" { self.session_id != None }
            update {
                self.active_fence_token = None;
                self.current_run_id = None;
            }
            to Idle
            emit InitiateRecycle
        }
        transition RecycleFromAttached {
            on input Recycle
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {
                self.active_fence_token = None;
                self.current_run_id = None;
            }
            to Attached
            emit InitiateRecycle
        }

        // =====================================================================
        // Absorbed substate transitions — Input Lifecycle
        // =====================================================================

        // RecoverInputLifecycle: restore persisted input lifecycle facts
        // through machine authority. Store recovery may present an input at
        // any lifecycle phase, so this transition deliberately owns the
        // whole recovered projection instead of replaying admit/stage/apply
        // transitions whose preconditions may no longer hold after restart.
        transition RecoverInputLifecycle {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RecoverInputLifecycle {
                input_id,
                phase,
                terminal_kind,
                superseded_by,
                aggregate_id,
                abandon_reason,
                abandon_attempt_count,
                attempt_count,
                run_id,
                boundary_sequence,
                lane
            }
            update {
                self.input_phases.insert(input_id, phase);

                if terminal_kind != None {
                    self.input_terminal_kind.insert(input_id, terminal_kind.get("value"));
                } else {
                    self.input_terminal_kind.remove(input_id);
                }

                if superseded_by != None {
                    self.input_superseded_by.insert(input_id, superseded_by.get("value"));
                } else {
                    self.input_superseded_by.remove(input_id);
                }

                if aggregate_id != None {
                    self.input_aggregate_id.insert(input_id, aggregate_id.get("value"));
                } else {
                    self.input_aggregate_id.remove(input_id);
                }

                if abandon_reason != None {
                    self.input_abandon_reason.insert(input_id, abandon_reason.get("value"));
                    self.input_abandon_attempt_count.insert(input_id, abandon_attempt_count);
                } else {
                    self.input_abandon_reason.remove(input_id);
                    self.input_abandon_attempt_count.remove(input_id);
                }

                self.input_attempt_counts.insert(input_id, attempt_count);

                if run_id != None {
                    self.input_run_associations.insert(input_id, run_id.get("value"));
                } else {
                    self.input_run_associations.remove(input_id);
                }

                if boundary_sequence != None {
                    self.input_boundary_sequences.insert(input_id, boundary_sequence.get("value"));
                } else {
                    self.input_boundary_sequences.remove(input_id);
                }

                if !self.input_admission_seq.contains_key(input_id) {
                    self.input_admission_seq.insert(input_id, self.next_admission_seq);
                    self.next_admission_seq += 1;
                }

                if lane != None {
                    self.input_lane.insert(input_id, lane.get("value"));
                } else {
                    self.input_lane.remove(input_id);
                }
            }
            to Idle
            emit InputLifecycleNotice
        }

        // QueueAccepted: admit a new input into the queue lane
        transition QueueAccepted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input QueueAccepted { input_id }
            guard "not_already_tracked" { !self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Queued);
                self.input_lane.insert(input_id, InputLane::Queue);
                self.input_admission_seq.insert(input_id, self.next_admission_seq);
                self.next_admission_seq += 1;
            }
            to Idle
            emit IngressAccepted
        }

        // SteerAccepted: admit a new input into the steer lane.
        // Symmetric sibling of QueueAccepted; the caller chooses lane based
        // on the input's resolved HandlingMode. Mutually exclusive by
        // construction because `input_lane` is a map, not two sets.
        transition SteerAccepted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SteerAccepted { input_id }
            guard "not_already_tracked" { !self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Queued);
                self.input_lane.insert(input_id, InputLane::Steer);
                self.input_admission_seq.insert(input_id, self.next_admission_seq);
                self.next_admission_seq += 1;
            }
            to Idle
            emit IngressAccepted
        }

        // ChangeLane: move a tracked input between Queue and Steer lanes.
        // Used for priority-front enqueue paths where the lane may differ
        // from the admission default. Because `input_lane` is a map,
        // insertion overwrites the prior lane — no strip-then-insert dance.
        transition ChangeLane {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ChangeLane { input_id, new_lane }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_lane.insert(input_id, new_lane);
            }
            to Idle
        }

        // StageForRun: stage a queued input for a run. Removes the input
        // from its work lane — staged inputs are no longer "currently
        // queuable". The shell's queue projections are derived from
        // `input_lane` membership and must not include staged inputs.
        transition StageForRun {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StageForRun { input_id, run_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Staged);
                self.input_run_associations.insert(input_id, run_id);
                self.input_lane.remove(input_id);
            }
            to Idle
            emit RecordRunAssociation
        }

        // IncrementAttemptCount: count this stage attempt for an input.
        transition IncrementAttemptCount {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input IncrementAttemptCount { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_attempt_counts.increment(input_id, 1);
            }
            to Idle
        }

        // RollbackStaged: return a staged input to queued. The caller
        // supplies the target lane (the shell's `HandlingMode` at rollback
        // time) so the DSL can re-admit the input to its work lane without
        // external post-hoc writes.
        transition RollbackStaged {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RollbackStaged { input_id, lane }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Queued);
                self.input_run_associations.remove(input_id);
                self.input_lane.insert(input_id, lane);
            }
            to Idle
            emit InputLifecycleNotice
        }

        // MarkApplied: mark an input as applied
        transition MarkApplied {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input MarkApplied { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Applied);
            }
            to Idle
            emit InputLifecycleNotice
        }

        // MarkAppliedPendingConsumption: Applied → AppliedPendingConsumption
        transition MarkAppliedPendingConsumption {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input MarkAppliedPendingConsumption { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::AppliedPendingConsumption);
            }
            to Idle
            emit InputLifecycleNotice
        }

        // ConsumeOnAccept: direct Accepted → Consumed (skip queue)
        transition ConsumeOnAccept {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ConsumeOnAccept { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Consumed);
                self.input_lane.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // RecordBoundarySeq: record boundary sequence for crash recovery
        transition RecordBoundarySeq {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RecordBoundarySeq { input_id, seq }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_boundary_sequences.insert(input_id, seq);
            }
            to Idle
            emit RecordBoundarySequence
        }

        // ConsumeInput: terminal — mark input consumed, remove from lanes
        transition ConsumeInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ConsumeInput { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Consumed);
                self.input_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, InputTerminalKind::Consumed);
                self.input_superseded_by.remove(input_id);
                self.input_aggregate_id.remove(input_id);
                self.input_abandon_reason.remove(input_id);
                self.input_abandon_attempt_count.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // SupersedeInput: terminal — mark input superseded, remove from
        // its work lane (either Queue or Steer; the unified map handles
        // both cases with a single remove).
        transition SupersedeInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SupersedeInput { input_id, superseded_by }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Superseded);
                self.input_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, InputTerminalKind::Superseded);
                self.input_superseded_by.insert(input_id, superseded_by);
                self.input_aggregate_id.remove(input_id);
                self.input_abandon_reason.remove(input_id);
                self.input_abandon_attempt_count.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // CoalesceInput: terminal — mark input coalesced, remove from
        // its work lane.
        transition CoalesceInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CoalesceInput { input_id, aggregate_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Coalesced);
                self.input_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, InputTerminalKind::Coalesced);
                self.input_aggregate_id.insert(input_id, aggregate_id);
                self.input_superseded_by.remove(input_id);
                self.input_abandon_reason.remove(input_id);
                self.input_abandon_attempt_count.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // AbandonInput: terminal — mark input abandoned, remove from lanes
        transition AbandonInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbandonInput { input_id, reason, attempt_count }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, InputPhase::Abandoned);
                self.input_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, InputTerminalKind::Abandoned);
                self.input_abandon_reason.insert(input_id, reason);
                self.input_abandon_attempt_count.insert(input_id, attempt_count);
                self.input_superseded_by.remove(input_id);
                self.input_aggregate_id.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // =====================================================================
        // Absorbed substate transitions — Ops Lifecycle
        // =====================================================================

        // RegisterOp: register a new operation as Provisioning
        transition RegisterOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RegisterOp { operation_id, kind }
            guard "not_already_registered" { !self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Provisioning);
                self.op_kinds.insert(operation_id, kind);
                self.op_peer_ready.insert(operation_id, false);
                self.op_progress_counts.insert(operation_id, 0);
                self.active_op_count += 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // StartOp: Provisioning -> Running.
        transition StartOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StartOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Provisioning)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Running);
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // CompleteOp: terminal success from Running|Retiring.
        transition CompleteOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CompleteOp { operation_id, outcome, payload }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Completed);
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.op_terminal_payload.insert(operation_id, payload);
                self.active_op_count -= 1;
                self.op_completion_seq.insert(operation_id, self.next_completion_seq);
                self.next_completion_seq += 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // FailOp: terminal failure from any non-terminal state.
        // Shell callers: `provisioning_failed` (only Provisioning) and
        // `fail_operation` (Provisioning|Running|Retiring). The DSL guard
        // allows the union; the shell exposes distinct entry points for
        // callers that only care about one sub-state.
        transition FailOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input FailOp { operation_id, outcome, payload }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Provisioning)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Failed);
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.op_terminal_payload.insert(operation_id, payload);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // CancelOp: terminal cancellation from any non-terminal state.
        transition CancelOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CancelOp { operation_id, outcome, payload }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Provisioning)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Cancelled);
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.op_terminal_payload.insert(operation_id, payload);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // AbortOp: terminal abort from Provisioning only.
        // The sole shell caller (`abort_provisioning`) aborts a spawn-in-flight
        // op; the DSL guard narrows legality to `Provisioning` to keep the
        // semantic boundary crisp.
        transition AbortOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortOp { operation_id, outcome, payload }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Provisioning)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Aborted);
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.op_terminal_payload.insert(operation_id, payload);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // PeerReadyOp: mark operation's peer as ready.
        //
        // The "only MobMemberChild ops expose a peer handoff" and
        // "only once per op" decisions live in the DSL as guards, not in
        // the shell. The shell classifies a guard rejection on
        // `kind_is_mob_member_child` back into
        // `OpsLifecycleError::PeerNotExpected`, and a rejection on
        // `not_already_peer_ready` into `OpsLifecycleError::AlreadyPeerReady`
        // via `classify_peer_ready_rejection`.
        transition PeerReadyOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerReadyOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "kind_is_mob_member_child" {
                self.op_kinds.get_copied(operation_id) == Some(OperationKind::MobMemberChild)
            }
            guard "not_already_peer_ready" {
                self.op_peer_ready.get_copied(operation_id) != Some(true)
            }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_peer_ready.insert(operation_id, true);
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // ProgressReportedOp: progress tick (Running|Retiring only).
        transition ProgressReportedOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ProgressReportedOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_progress_counts.increment(operation_id, 1);
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // RetireRequestedOp: Running -> Retiring (non-terminal).
        transition RetireRequestedOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RetireRequestedOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Retiring);
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // RetireCompletedOp: terminal retirement (Running|Retiring -> Retired).
        transition RetireCompletedOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RetireCompletedOp { operation_id, outcome, payload }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Retired);
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.op_terminal_payload.insert(operation_id, payload);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // TerminateOp: bulk-terminate variant used by owner-terminated cascade.
        // Shell loops across non-terminal ops (mechanical cursor) and issues
        // one TerminateOp per op; the session lock provides cascade atomicity.
        // The "is this op terminal-able?" decision lives in the DSL guard —
        // terminal-state TerminateOp is a guard rejection, surfaced to the
        // caller via `classify_op_rejection` as `InvalidTransition`.
        transition TerminateOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input TerminateOp { operation_id, outcome, payload }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            guard "from_status_valid" {
                self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Provisioning)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Running)
                || self.op_statuses.get_copied(operation_id) == Some(OperationStatus::Retiring)
            }
            update {
                self.op_statuses.insert(operation_id, OperationStatus::Terminated);
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.op_terminal_payload.insert(operation_id, payload);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // RequestWaitAll: activate wait-all barrier with explicit membership.
        // `wait_request_id`, `wait_operation_ids`, and the typed
        // `wait_operation_id_tokens` handoff payload are DSL-owned; shell must
        // not mirror them as canonical truth.
        transition RequestWaitAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequestWaitAll { wait_request_id, operation_ids, operation_id_tokens }
            update {
                self.wait_active = true;
                self.wait_request_id = Some(wait_request_id);
                self.wait_operation_ids = operation_ids;
                self.wait_operation_id_tokens = operation_id_tokens;
            }
            to Idle
        }

        // SatisfyWaitAll: deactivate wait-all barrier once every member of
        // `wait_operation_ids` has reached a terminal status. The
        // "all members terminal" decision lives in the DSL guard — the
        // shell no longer pre-computes the verdict before firing this input.
        // A guard rejection under `wait_is_active` means the barrier wasn't
        // active; a rejection under `all_members_terminal` means the shell
        // fired early. Callers drive satisfaction by firing this input on
        // each op terminalization; the guard serves as the fixed point.
        transition SatisfyWaitAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SatisfyWaitAll { wait_request_id, operation_id_tokens }
            guard "wait_is_active" { self.wait_active == true }
            guard "wait_request_matches" { self.wait_request_id == Some(wait_request_id) }
            guard "operation_tokens_match" { self.wait_operation_id_tokens == operation_id_tokens }
            guard "all_members_terminal" {
                for_all(member_id in self.wait_operation_ids,
                    self.op_statuses.get_copied(member_id) == Some(OperationStatus::Completed)
                    || self.op_statuses.get_copied(member_id) == Some(OperationStatus::Failed)
                    || self.op_statuses.get_copied(member_id) == Some(OperationStatus::Aborted)
                    || self.op_statuses.get_copied(member_id) == Some(OperationStatus::Cancelled)
                    || self.op_statuses.get_copied(member_id) == Some(OperationStatus::Retired)
                    || self.op_statuses.get_copied(member_id) == Some(OperationStatus::Terminated))
            }
            update {
                self.wait_active = false;
                self.wait_request_id = None;
                self.wait_operation_ids = EmptySet;
                self.wait_operation_id_tokens = EmptySet;
            }
            to Idle
            emit WaitAllSatisfied {
                wait_request_id: wait_request_id,
                operation_ids: operation_id_tokens
            }
        }

        // CancelWaitAll: clear the wait-all barrier without the terminality
        // check. Fired by the shell on `wait_all` future drop; the
        // satisfaction obligation is NOT emitted because no authority
        // resolved the request. Idempotent on an already-cleared barrier.
        transition CancelWaitAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CancelWaitAll
            guard "wait_is_active" { self.wait_active == true }
            update {
                self.wait_active = false;
                self.wait_request_id = None;
                self.wait_operation_ids = EmptySet;
                self.wait_operation_id_tokens = EmptySet;
            }
            to Idle
        }

        // =====================================================================
        // Absorbed substate transitions — Comms Drain
        // =====================================================================

        // SpawnDrain: start a drain task
        transition SpawnDrain {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SpawnDrain { mode }
            guard "drain_can_spawn" { self.drain_phase == DrainPhase::Inactive || self.drain_phase == DrainPhase::Stopped || self.drain_phase == DrainPhase::ExitedRespawnable }
            update {
                self.drain_phase = DrainPhase::Running;
                self.drain_mode = Some(mode);
            }
            to Idle
            emit SpawnDrainTask
        }

        // StopDrain: stop the running drain
        transition StopDrain {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StopDrain
            guard "drain_is_running" { self.drain_phase == DrainPhase::Running }
            update {
                self.drain_phase = DrainPhase::Stopped;
            }
            to Idle
        }

        // DrainExitedClean: drain exited cleanly, reset to inactive
        transition DrainExitedClean {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DrainExitedClean
            update {
                self.drain_phase = DrainPhase::Inactive;
                self.drain_mode = None;
            }
            to Idle
        }

        // DrainExitedRespawnable: drain exited but can be respawned
        transition DrainExitedRespawnable {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DrainExitedRespawnable
            update {
                self.drain_phase = DrainPhase::ExitedRespawnable;
            }
            to Idle
        }

        // =====================================================================
        // Absorbed substate transitions — Visibility
        // =====================================================================

        // StageVisibilityFilter: stage a new tool filter; DSL mints the new
        // staged-revision token via `next_staged_visibility_revision`
        // (dogma round 4, wave 2b #12 — single-owner monotonic). The shell
        // reads the minted value back via
        // `MeerkatMachine::stage_session_dsl_input_with_minted_revision`.
        transition StageVisibilityFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StageVisibilityFilter { filter }
            update {
                self.next_staged_visibility_revision = self.next_staged_visibility_revision + 1;
                self.staged_filter = filter;
                self.staged_visibility_revision = self.next_staged_visibility_revision;
            }
            to Idle
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
        }

        // CommitVisibilityFilter: promote staged filter to active
        transition CommitVisibilityFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CommitVisibilityFilter { filter, revision }
            update {
                self.active_filter = filter;
                self.active_visibility_revision = revision;
            }
            to Idle
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
        }

        // StageDeferredNames: stage a set of deferred tool names; DSL mints
        // the new staged-revision token via `next_staged_visibility_revision`
        // (dogma round 4, wave 2b #12). Shell reads the minted value back.
        transition StageDeferredNames {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StageDeferredNames { names }
            guard "deferred_names_empty" { names == EmptySet }
            update {
                self.next_staged_visibility_revision = self.next_staged_visibility_revision + 1;
                self.staged_deferred_names = names;
                self.staged_deferred_authorities = EmptyMap;
                self.staged_visibility_revision = self.next_staged_visibility_revision;
            }
            to Idle
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
        }

        // CommitDeferredNames: promote staged deferred authority to active
        transition CommitDeferredNames {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CommitDeferredNames { authorities }
            guard "deferred_authorities_have_identity" {
                deferred_authorities_have_identity(authorities.keys(), authorities)
            }
            update {
                self.active_deferred_names = authorities.keys();
                self.active_deferred_authorities = authorities;
            }
            to Idle
            emit RefreshVisibleSurfaceSet { snapshot_epoch: self.snapshot_epoch }
        }

        // SyncVisibilityRevisions: external-install water-mark reconciliation.
        // Fired by the shell whenever an external durable visibility state is
        // installed (recovery, cross-session hot-swap, LLM reconfigure). Keeps
        // the DSL monotonic counter honest against externally-minted revisions
        // so subsequent `StageVisibilityFilter` / `StageDeferredNames` mints
        // continue advancing from the high-water mark rather than the DSL's
        // local 0.
        //
        // Typed idempotence via guard: the transition fires only when at
        // least one of the installed revisions exceeds the counter. When
        // both revisions are at or below the counter (e.g., fresh-build
        // with default-zero visibility state on a never-advanced counter,
        // or recovery replay of a state the DSL already reflects), the
        // guard rejects and the caller sees `Ok(false)` — no shell-side
        // input-value pre-check, no silent no-op update.
        transition SyncVisibilityRevisions {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SyncVisibilityRevisions {
                active_revision, staged_revision,
                active_deferred_names, staged_deferred_names,
                active_deferred_authorities, staged_deferred_authorities
            }
            guard "counter_advances" {
                active_revision > self.next_staged_visibility_revision
                || staged_revision > self.next_staged_visibility_revision
            }
            guard "active_deferred_authorities_cover_names" {
                for_all(requested_name in active_deferred_names, active_deferred_authorities.contains_key(requested_name))
            }
            guard "staged_deferred_authorities_cover_names" {
                for_all(requested_name in staged_deferred_names, staged_deferred_authorities.contains_key(requested_name))
            }
            guard "active_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in active_deferred_authorities.keys(), active_deferred_names.contains(witnessed_name))
            }
            guard "staged_deferred_authorities_are_name_scoped" {
                for_all(witnessed_name in staged_deferred_authorities.keys(), staged_deferred_names.contains(witnessed_name))
            }
            guard "active_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(active_deferred_names, active_deferred_authorities)
            }
            guard "staged_deferred_authorities_have_identity" {
                deferred_authorities_have_identity(staged_deferred_names, staged_deferred_authorities)
            }
            update {
                self.active_deferred_names = active_deferred_names;
                self.staged_deferred_names = staged_deferred_names;
                self.active_deferred_authorities = active_deferred_authorities;
                self.staged_deferred_authorities = staged_deferred_authorities;
                if active_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = active_revision;
                }
                if staged_revision > self.next_staged_visibility_revision {
                    self.next_staged_visibility_revision = staged_revision;
                }
            }
            to Idle
        }

        // =====================================================================
        // Realtime-attachment transitions
        // =====================================================================

        transition ProjectRealtimeIntent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ProjectRealtimeIntent { present }
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_intent_present = present;
            }
            to Idle
            emit RealtimeIntentProjected { present: present }
        }

        transition BeginRealtimeBinding {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BeginRealtimeBinding
            guard "session_registered" { self.session_id != None }
            guard "no_topology_reconfigure_in_progress" { self.live_topology_phase == LiveTopologyPhase::Idle }
            update {
                self.realtime_binding_state = RealtimeBindingState::BindingNotReady;
                self.realtime_binding_authority_epoch = Some(self.realtime_next_authority_epoch);
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
            emit RealtimeBindingRotated { authority_epoch: self.realtime_binding_authority_epoch.get("value") }
        }

        transition ReplaceRealtimeBinding {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ReplaceRealtimeBinding
            guard "session_registered" { self.session_id != None }
            guard "no_topology_reconfigure_in_progress" { self.live_topology_phase == LiveTopologyPhase::Idle }
            update {
                self.realtime_binding_state = RealtimeBindingState::ReplacementPending;
                self.realtime_binding_authority_epoch = Some(self.realtime_next_authority_epoch);
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
            emit RealtimeBindingRotated { authority_epoch: self.realtime_binding_authority_epoch.get("value") }
        }

        transition DetachRealtimeBinding {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DetachRealtimeBinding
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
        }

        transition RequireRealtimeReattach {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequireRealtimeReattach
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = true;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
        }

        transition RequireRealtimeReattachForAuthority {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequireRealtimeReattachForAuthority { authority_epoch }
            guard "session_registered" { self.session_id != None }
            guard "authority_matches_current" { self.realtime_binding_authority_epoch == Some(authority_epoch) }
            update {
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = true;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
        }

        transition PublishRealtimeSignal {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PublishRealtimeSignal { authority_epoch, next_binding_state }
            guard "authority_matches_current" { self.realtime_binding_authority_epoch == Some(authority_epoch) }
            guard "no_topology_reconfigure_in_progress" { self.live_topology_phase == LiveTopologyPhase::Idle }
            guard "valid_next_state" {
                next_binding_state == RealtimeBindingState::BindingNotReady
                || next_binding_state == RealtimeBindingState::BindingReady
                || next_binding_state == RealtimeBindingState::ReplacementPending
            }
            update {
                self.realtime_binding_state = next_binding_state;
                self.realtime_reattach_required = false;
                if next_binding_state == RealtimeBindingState::BindingReady {
                    self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                    self.realtime_reconnect_attempt_count = 0;
                    self.realtime_reconnect_next_retry_at_ms = None;
                    self.realtime_reconnect_deadline_at_ms = None;
                }
            }
            to Idle
        }

        transition BeginRealtimeReconnectCycle {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BeginRealtimeReconnectCycle { next_retry_at_ms, deadline_at_ms }
            guard "session_registered" { self.session_id != None }
            guard "reattach_required" { self.realtime_reattach_required }
            guard "cycle_idle" { self.realtime_reconnect_cycle_state == RealtimeReconnectCycleState::Idle }
            update {
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Reconnecting;
                self.realtime_reconnect_attempt_count = 1;
                self.realtime_reconnect_next_retry_at_ms = next_retry_at_ms;
                self.realtime_reconnect_deadline_at_ms = deadline_at_ms;
            }
            to Idle
            emit RealtimeReconnectProgressProjected {
                attempt_count: 1,
                next_retry_at_ms: next_retry_at_ms,
                deadline_at_ms: deadline_at_ms,
            }
        }

        transition ScheduleRealtimeReconnectRetry {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ScheduleRealtimeReconnectRetry { next_retry_at_ms }
            guard "session_registered" { self.session_id != None }
            guard "cycle_reconnecting" { self.realtime_reconnect_cycle_state == RealtimeReconnectCycleState::Reconnecting }
            update {
                self.realtime_reconnect_attempt_count = self.realtime_reconnect_attempt_count + 1;
                self.realtime_reconnect_next_retry_at_ms = next_retry_at_ms;
            }
            to Idle
            emit RealtimeReconnectProgressProjected {
                attempt_count: self.realtime_reconnect_attempt_count,
                next_retry_at_ms: next_retry_at_ms,
                deadline_at_ms: self.realtime_reconnect_deadline_at_ms,
            }
        }

        transition ExhaustRealtimeReconnectCycle {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ExhaustRealtimeReconnectCycle
            guard "session_registered" { self.session_id != None }
            guard "cycle_reconnecting" { self.realtime_reconnect_cycle_state == RealtimeReconnectCycleState::Reconnecting }
            update {
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Exhausted;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
            emit RealtimeReconnectProgressProjected {
                attempt_count: 0,
                next_retry_at_ms: None,
                deadline_at_ms: None,
            }
        }

        transition ClearRealtimeReconnectProgress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ClearRealtimeReconnectProgress
            guard "session_registered" { self.session_id != None }
            update {
                self.realtime_reconnect_cycle_state = RealtimeReconnectCycleState::Idle;
                self.realtime_reconnect_attempt_count = 0;
                self.realtime_reconnect_next_retry_at_ms = None;
                self.realtime_reconnect_deadline_at_ms = None;
            }
            to Idle
            emit RealtimeReconnectProgressProjected {
                attempt_count: 0,
                next_retry_at_ms: None,
                deadline_at_ms: None,
            }
        }

        // =====================================================================
        // Live-topology reconfigure transitions
        // =====================================================================

        transition BeginLiveTopologyReconfigure {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BeginLiveTopologyReconfigure { authority_epoch }
            guard "session_registered" { self.session_id != None }
            guard "authority_matches_current" { self.realtime_binding_authority_epoch == Some(authority_epoch) }
            guard "topology_idle" { self.live_topology_phase == LiveTopologyPhase::Idle }
            update {
                self.live_topology_phase = LiveTopologyPhase::Reconfiguring;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        // MarkLiveTopologyDetached is the "safe to detach" gate. It rejects
        // while the runtime is mid-primitive (turn_phase != Ready && !=
        // DrainingBoundary). A shell retry loop applies this input until the
        // DSL accepts, encoding the "wait for next natural boundary" invariant
        // at the DSL layer rather than in shell polling of current_run_id.
        //
        // **Catalog/runtime divergence (intentional):** the catalog DSL
        // (`meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs`) does
        // not model `turn_phase` and conservatively guards on
        // `current_run_id == None` instead. That is a strict over-
        // approximation of this guard: every catalog trace is admissible
        // here, so invariants proven by TLC against the catalog hold in
        // production. The runtime can additionally detach at
        // `DrainingBoundary` mid-run, which TLC does not exercise.
        transition MarkLiveTopologyDetached {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input MarkLiveTopologyDetached
            guard "session_registered" { self.session_id != None }
            guard "topology_reconfiguring" { self.live_topology_phase == LiveTopologyPhase::Reconfiguring }
            guard "turn_at_safe_boundary" {
                self.turn_phase == TurnPhase::Ready
                || self.turn_phase == TurnPhase::DrainingBoundary
                || self.turn_phase == TurnPhase::Completed
                || self.turn_phase == TurnPhase::Failed
                || self.turn_phase == TurnPhase::Cancelled
            }
            update {
                self.live_topology_phase = LiveTopologyPhase::Detached;
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = false;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition ApplyLiveTopologyIdentity {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ApplyLiveTopologyIdentity
            guard "session_registered" { self.session_id != None }
            guard "topology_detached" { self.live_topology_phase == LiveTopologyPhase::Detached }
            update {
                self.live_topology_phase = LiveTopologyPhase::HostIdentityApplied;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition ApplyLiveTopologyVisibility {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ApplyLiveTopologyVisibility
            guard "session_registered" { self.session_id != None }
            guard "host_identity_applied" { self.live_topology_phase == LiveTopologyPhase::HostIdentityApplied }
            update {
                self.live_topology_phase = LiveTopologyPhase::HostVisibilityApplied;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition CompleteLiveTopology {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CompleteLiveTopology
            guard "session_registered" { self.session_id != None }
            guard "host_visibility_applied" { self.live_topology_phase == LiveTopologyPhase::HostVisibilityApplied }
            update {
                self.live_topology_phase = LiveTopologyPhase::Idle;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition AbortLiveTopologyBeforeDetach {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortLiveTopologyBeforeDetach
            guard "session_registered" { self.session_id != None }
            guard "topology_reconfiguring" { self.live_topology_phase == LiveTopologyPhase::Reconfiguring }
            update {
                self.live_topology_phase = LiveTopologyPhase::Idle;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        transition FailLiveTopologyAfterDetach {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input FailLiveTopologyAfterDetach
            guard "session_registered" { self.session_id != None }
            guard "topology_past_detach" {
                self.live_topology_phase == LiveTopologyPhase::Detached
                || self.live_topology_phase == LiveTopologyPhase::HostIdentityApplied
                || self.live_topology_phase == LiveTopologyPhase::HostVisibilityApplied
            }
            update {
                self.live_topology_phase = LiveTopologyPhase::Idle;
                self.realtime_binding_state = RealtimeBindingState::Unbound;
                self.realtime_binding_authority_epoch = None;
                self.realtime_reattach_required = true;
                self.realtime_next_authority_epoch = self.realtime_next_authority_epoch + 1;
            }
            to Idle
            emit LiveTopologyPhaseChanged
        }

        // =====================================================================
        // MCP server lifecycle transitions
        // =====================================================================
        //
        // Each MCP server is keyed by its configured `McpServerId` in the
        // `mcp_server_states` map. The shell translates incoming connection
        // events into these inputs; each input rewrites that key's state and
        // emits `McpServerStateChanged` so downstream consumers (the
        // `[MCP_PENDING]` system-notice toggle and tool-availability filter)
        // can stay pure reads off DSL state.

        transition McpServerConnectPending {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerConnectPending { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::PendingConnect);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::PendingConnect }
        }

        transition McpServerConnected {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerConnected { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::Connected);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::Connected }
        }

        transition McpServerFailed {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerFailed { server_id, error }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::Failed);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::Failed }
        }

        transition McpServerDisconnected {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerDisconnected { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                self.mcp_server_states.insert(server_id, McpServerState::Disconnected);
            }
            to Idle
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::Disconnected }
        }

        transition McpServerReload {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input McpServerReload { server_id }
            guard "session_registered" { self.session_id != None }
            update {
                // Reload moves the server back to PendingConnect; the shell
                // tears down the prior connection and drives a fresh
                // Connected / Failed transition on completion.
                self.mcp_server_states.insert(server_id, McpServerState::PendingConnect);
            }
            to Idle
            emit McpServerReloadRequested { server_id: server_id }
            emit McpServerStateChanged { server_id: server_id, new_state: McpServerState::PendingConnect }
        }

        // =====================================================================
        // Peer interaction lifecycle transitions (W1-A / issue #264)
        // =====================================================================
        //
        // The shell fires these inputs on outbound peer-request send, progress
        // / terminal response arrival, timeouts, and the mirror inbound
        // lifecycle. Terminal-shaped transitions (`*Terminal`, `*TimedOut`,
        // `Replied`) remove their corr_id from the state map and emit
        // `PeerInteractionCleanup` / `InboundPeerInteractionStateChanged`,
        // so subsequent double-terminal inputs are rejected at the
        // `contains_key` guard — the projection invariant
        // "channel live iff corr_id ∈ pending" is enforced by the DSL, not by
        // shell discipline.

        transition PeerRequestSent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerRequestSent { corr_id, to }
            guard "not_already_pending" { !self.pending_peer_requests.contains_key(corr_id) }
            update {
                self.pending_peer_requests.insert(corr_id, OutboundPeerRequestState::Sent);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::Sent }
        }

        transition PeerResponseProgressArrived {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseProgressArrived { corr_id }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            update {
                self.pending_peer_requests.insert(corr_id, OutboundPeerRequestState::AcceptedProgress);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::AcceptedProgress }
        }

        transition PeerResponseTerminalArrivedCompleted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseTerminalArrived { corr_id, disposition }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            guard "completed" { disposition == PeerTerminalDisposition::Completed }
            update {
                self.pending_peer_requests.remove(corr_id);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::Completed }
            emit PeerInteractionCleanup { corr_id: corr_id }
        }

        transition PeerResponseTerminalArrivedFailed {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseTerminalArrived { corr_id, disposition }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            guard "failed" { disposition == PeerTerminalDisposition::Failed }
            update {
                self.pending_peer_requests.remove(corr_id);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::Failed }
            emit PeerInteractionCleanup { corr_id: corr_id }
        }

        transition PeerRequestTimedOut {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerRequestTimedOut { corr_id }
            guard "pending_exists" { self.pending_peer_requests.contains_key(corr_id) }
            update {
                self.pending_peer_requests.remove(corr_id);
            }
            to Idle
            emit PeerInteractionStateChanged { corr_id: corr_id, new_state: OutboundPeerRequestState::TimedOut }
            emit PeerInteractionCleanup { corr_id: corr_id }
        }

        transition PeerRequestReceived {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerRequestReceived { corr_id }
            guard "not_already_inbound" { !self.inbound_peer_requests.contains_key(corr_id) }
            update {
                self.inbound_peer_requests.insert(corr_id, InboundPeerRequestState::Received);
            }
            to Idle
            emit InboundPeerInteractionStateChanged { corr_id: corr_id, new_state: InboundPeerRequestState::Received }
        }

        transition PeerResponseReplied {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerResponseReplied { corr_id }
            guard "inbound_exists" { self.inbound_peer_requests.contains_key(corr_id) }
            update {
                self.inbound_peer_requests.remove(corr_id);
            }
            to Idle
            emit InboundPeerInteractionStateChanged { corr_id: corr_id, new_state: InboundPeerRequestState::Replied }
        }

        // Session-context advancement transition (W2-E / issue #264).
        //
        // Fires at every shell site that mutates canonical session truth —
        // prompt append, external content injection, tool-result append,
        // external assistant output, runtime-system-context append, and the
        // tail of any turn that advances the session's `updated_at`
        // watermark. Monotonic: the guard rejects any advance whose
        // `updated_at_ms` is not strictly greater than the last recorded
        // watermark. This lets the shell apply the transition
        // unconditionally post-mutation; duplicate or out-of-order ticks are
        // filtered by the DSL, not by the caller.
        //
        // Advancing transitions emit `SessionContextAdvanced { updated_at_ms }`
        // which the realtime projection consumer reads via an installed
        // `SessionContextAdvancedObserver` to drive its typed
        // `ProjectionFreshness` state. No polling, no watch channel, no
        // hand-maintained `projection_refresh_dirty` flag.
        transition AdvanceSessionContext {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AdvanceSessionContext { updated_at_ms }
            guard "monotonic" { updated_at_ms > self.last_session_context_updated_at_ms }
            update {
                self.last_session_context_updated_at_ms = updated_at_ms;
            }
            to Idle
            emit SessionContextAdvanced { updated_at_ms: updated_at_ms }
        }

        // =====================================================================
        // Interaction stream lifecycle transitions (U6 / dogma #5)
        // =====================================================================
        //
        // The shell fires these inputs on reservation, attach, terminal
        // completion, TTL expiry, and consumer drop. Active (non-terminal)
        // states stay in the map; terminal transitions remove the entry and
        // emit `InteractionStreamCleanup` so the shell drops the matching
        // channel projection. Re-attach after terminal is rejected by the
        // `not_already_present` / `is_reserved` guards.

        transition InteractionStreamReserved {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamReserved { corr_id }
            guard "not_reserved" { !self.reserved_interaction_streams.contains(corr_id) }
            guard "not_attached" { !self.attached_interaction_streams.contains(corr_id) }
            update {
                self.reserved_interaction_streams.insert(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Reserved }
        }

        transition InteractionStreamAttached {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamAttached { corr_id }
            guard "is_reserved" { self.reserved_interaction_streams.contains(corr_id) }
            update {
                self.reserved_interaction_streams.remove(corr_id);
                self.attached_interaction_streams.insert(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Attached }
        }

        transition InteractionStreamCompleted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamCompleted { corr_id }
            guard "is_attached" { self.attached_interaction_streams.contains(corr_id) }
            update {
                self.attached_interaction_streams.remove(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Completed }
            emit InteractionStreamCleanup { corr_id: corr_id }
        }

        transition InteractionStreamExpired {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamExpired { corr_id }
            guard "is_reserved" { self.reserved_interaction_streams.contains(corr_id) }
            update {
                self.reserved_interaction_streams.remove(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::Expired }
            emit InteractionStreamCleanup { corr_id: corr_id }
        }

        transition InteractionStreamClosedEarly {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input InteractionStreamClosedEarly { corr_id }
            guard "is_attached" { self.attached_interaction_streams.contains(corr_id) }
            update {
                self.attached_interaction_streams.remove(corr_id);
            }
            to Idle
            emit InteractionStreamStateChanged { corr_id: corr_id, new_state: InteractionStreamState::ClosedEarly }
            emit InteractionStreamCleanup { corr_id: corr_id }
        }

        // =====================================================================
        // Realtime product-turn lifecycle (U9 / dogma #4)
        // =====================================================================
        //
        // Five-phase lifecycle: Idle → AwaitingProgress → {Committed,
        // OutputStarted} → Preemptible → Idle. Each transition is guarded
        // on the source phase(s) for which it advances; idempotent fires
        // (e.g., `ProductTurnCommitted` when already `Committed` or
        // `Preemptible`) are guard-rejected and surfaced as `Ok(false)` by
        // the runtime handle, so the realtime-WS shell can fire
        // unconditionally on every observed provider-session event without
        // tracking its own phase.

        // Input accepted — only meaningful from Idle. In-flight inputs on
        // live turns do not rewind the phase.
        transition ProductTurnInFlight {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnInFlight
            guard "only_from_idle" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::AwaitingProgress;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::AwaitingProgress }
        }

        // `TurnCommitted` arrived from the provider. Valid from
        // AwaitingProgress (→ Committed) and OutputStarted (→ Preemptible).
        transition ProductTurnCommittedFromAwaiting {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnCommitted
            guard "from_awaiting" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::AwaitingProgress
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Committed;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Committed }
        }

        transition ProductTurnCommittedFromOutput {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnCommitted
            guard "from_output_started" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::OutputStarted
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Preemptible;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Preemptible }
        }

        // Output delta / tool call arrived. Valid from AwaitingProgress
        // (→ OutputStarted) and Committed (→ Preemptible).
        transition ProductOutputStartedFromAwaiting {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductOutputStarted
            guard "from_awaiting" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::AwaitingProgress
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::OutputStarted;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::OutputStarted }
        }

        transition ProductOutputStartedFromCommitted {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductOutputStarted
            guard "from_committed" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Committed
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Preemptible;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Preemptible }
        }

        // Interrupt clears the "output-started" milestone without tearing
        // down the turn. Preemptible → Committed; OutputStarted →
        // AwaitingProgress. Idempotent from Idle / AwaitingProgress /
        // Committed is handled by the monotonic pair below.
        transition ProductTurnInterruptedFromPreemptible {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnInterrupted
            guard "from_preemptible" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Preemptible
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Committed;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Committed }
        }

        transition ProductTurnInterruptedFromOutput {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnInterrupted
            guard "from_output_started" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::OutputStarted
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::AwaitingProgress;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::AwaitingProgress }
        }

        // Logical-turn terminal. Rewinds any active phase back to Idle.
        // Idempotent from Idle is rejected by the guard.
        transition ProductTurnTerminal {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ProductTurnTerminal
            guard "not_already_idle" {
                self.realtime_product_turn_phase != RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_product_turn_phase = RealtimeProductTurnPhase::Idle;
            }
            to Idle
            emit RealtimeProductTurnPhaseChanged { new_phase: RealtimeProductTurnPhase::Idle }
        }

        // =====================================================================
        // Realtime projection freshness (dogma round 2, U-C / dogma #1, #3, #13, #20)
        // =====================================================================
        //
        // Canonical freshness state for the realtime provider session's
        // projection relative to canonical session truth. Three transitions
        // split on product-turn phase + current freshness to decide the next
        // state; a fourth handles turn-end promotion.

        // Observer tick arrived while the product turn is live — record the
        // pending advance as `StaleDeferred` so barge-in continuity is
        // preserved. Monotonic: rejects advances that don't surpass the
        // current frontier.
        transition RealtimeProjectionAdvanceDuringTurn {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionAdvanceObserved { advanced_at_ms }
            guard "monotonic" { advanced_at_ms > self.realtime_projection_frontier_ms }
            guard "turn_in_flight" {
                self.realtime_product_turn_phase != RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::StaleDeferred;
                self.realtime_projection_frontier_ms = advanced_at_ms;
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::StaleDeferred,
                frontier_ms: advanced_at_ms
            }
        }

        // Observer tick arrived while the product turn is idle — record the
        // pending advance as `StaleImmediate` so the next drain site picks
        // it up.
        transition RealtimeProjectionAdvanceWhileIdle {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionAdvanceObserved { advanced_at_ms }
            guard "monotonic" { advanced_at_ms > self.realtime_projection_frontier_ms }
            guard "turn_idle" {
                self.realtime_product_turn_phase == RealtimeProductTurnPhase::Idle
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::StaleImmediate;
                self.realtime_projection_frontier_ms = advanced_at_ms;
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::StaleImmediate,
                frontier_ms: advanced_at_ms
            }
        }

        // After a successful provider-session refresh drain (or at an
        // own-turn commit that advanced the session-context watermark to
        // `observed_ms`). Returns to `Clean` ONLY when `observed_ms`
        // matches or exceeds the current frontier. If a concurrent
        // external advance (e.g. a peer_response_terminal landing while
        // our own turn was committing) already pushed the frontier above
        // `observed_ms` via a `RealtimeProjectionAdvanceObserved` tick,
        // the refresh is guard-rejected so the stale state at the higher
        // frontier is preserved — the external advance still owes a
        // refresh, and clobbering it here would drop the tick the next
        // drain site depends on. This is the DSL-owned successor of the
        // shell-side "preserve newer concurrent external advance" dance
        // #299 introduced on the pre-U-C W2-E freshness state.
        transition RealtimeProjectionRefreshed {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionRefreshed { observed_ms }
            guard "not_behind_frontier" {
                observed_ms >= self.realtime_projection_frontier_ms
            }
            guard "actually_changing" {
                self.realtime_projection_freshness != RealtimeProjectionFreshness::Clean
                || observed_ms > self.realtime_projection_frontier_ms
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::Clean;
                if observed_ms > self.realtime_projection_frontier_ms {
                    self.realtime_projection_frontier_ms = observed_ms;
                }
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::Clean,
                frontier_ms: self.realtime_projection_frontier_ms
            }
        }

        // Re-seed `Clean` baseline on product-session close / error /
        // reconnect. Monotonic in the same sense as `RealtimeProjectionRefreshed`:
        // `baseline_ms` must not regress the frontier. If a newer observer
        // tick transitioned the freshness to `StaleImmediate` / `StaleDeferred`
        // at a higher frontier between the caller's read and this fire, the
        // reset collapses to `Clean` at that higher frontier — the drain is
        // about to re-enter via the product-session rebuild anyway, and
        // regressing the frontier would drop a real advance.
        transition RealtimeProjectionReset {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input RealtimeProjectionReset { baseline_ms }
            guard "actually_changing" {
                self.realtime_projection_freshness != RealtimeProjectionFreshness::Clean
                || baseline_ms > self.realtime_projection_frontier_ms
            }
            update {
                self.realtime_projection_freshness = RealtimeProjectionFreshness::Clean;
                if baseline_ms > self.realtime_projection_frontier_ms {
                    self.realtime_projection_frontier_ms = baseline_ms;
                }
            }
            to Idle
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: RealtimeProjectionFreshness::Clean,
                frontier_ms: self.realtime_projection_frontier_ms
            }
        }

        // =====================================================================
        // Realtime reconnect policy (dogma round 2, U-C / dogma #1, #3, #18, #20)
        // =====================================================================
        //
        // The DSL classifies what a clean provider-session close means for
        // the realtime channel's reconnect behavior. Replaces the shell-local
        // boolean pair (`client_has_submitted_input`,
        // `last_turn_terminally_completed`).

        // Client submitted work to the provider session — any subsequent
        // clean close while this policy stands is a mid-work disconnect.
        // Also promotes `StaleDeferred → StaleImmediate` at turn end is
        // handled by `ClassifyRealtimeTurnTerminated`.
        transition ClassifyRealtimeClientInputSubmitted {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ClassifyRealtimeClientInputSubmitted
            guard "not_already_reattach" {
                self.realtime_reconnect_policy != RealtimeReconnectPolicy::ReattachAndRecover
            }
            update {
                self.realtime_reconnect_policy = RealtimeReconnectPolicy::ReattachAndRecover;
            }
            to Idle
            emit RealtimeReconnectPolicyChanged {
                new_policy: RealtimeReconnectPolicy::ReattachAndRecover
            }
        }

        // Mid-turn activity on the provider session (e.g. a provider-issued
        // tool call before a terminal turn completion) is not terminal, so
        // flag the reconnect policy back to `ReattachAndRecover` if a prior
        // terminal had already flipped us to `CleanExit`. Idempotent when
        // already `ReattachAndRecover`.
        transition ClassifyRealtimeMidTurnActivity {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ClassifyRealtimeMidTurnActivity
            guard "not_already_reattach" {
                self.realtime_reconnect_policy != RealtimeReconnectPolicy::ReattachAndRecover
            }
            update {
                self.realtime_reconnect_policy = RealtimeReconnectPolicy::ReattachAndRecover;
            }
            to Idle
            emit RealtimeReconnectPolicyChanged {
                new_policy: RealtimeReconnectPolicy::ReattachAndRecover
            }
        }

        // Logical turn reached a terminal stop reason — the session delivered
        // the client's requested work. A subsequent clean close is a session
        // finishing, not a mid-work drop. Also folds in the `StaleDeferred →
        // StaleImmediate` promotion so the turn-end drain site picks up any
        // pending async mutation.
        transition ClassifyRealtimeTurnTerminated {
            per_phase [Initializing, Idle, Attached, Running, Retired, Stopped]
            on input ClassifyRealtimeTurnTerminated
            guard "actually_changing" {
                self.realtime_reconnect_policy != RealtimeReconnectPolicy::CleanExit
                || self.realtime_projection_freshness == RealtimeProjectionFreshness::StaleDeferred
            }
            update {
                self.realtime_reconnect_policy = RealtimeReconnectPolicy::CleanExit;
                if self.realtime_projection_freshness == RealtimeProjectionFreshness::StaleDeferred {
                    self.realtime_projection_freshness = RealtimeProjectionFreshness::StaleImmediate;
                }
            }
            to Idle
            emit RealtimeReconnectPolicyChanged {
                new_policy: RealtimeReconnectPolicy::CleanExit
            }
            emit RealtimeProjectionFreshnessChanged {
                new_freshness: self.realtime_projection_freshness,
                frontier_ms: self.realtime_projection_frontier_ms
            }
        }

        // =====================================================================
        // Peer-ingress transport capability ownership (W2-G / issue #264)
        // =====================================================================

        // AttachSessionIngress: valid from `Unattached`, or as an exact
        // idempotent re-assertion of the already-owned session runtime.
        // Rejects `MobOwned` → `SessionOwned` silent downgrades by
        // construction.
        transition AttachSessionIngress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AttachSessionIngress { comms_runtime_id }
            guard "session_registered" { self.session_id != None }
            guard "owner_allows_session_attach" {
                self.peer_ingress_owner_kind == PeerIngressOwnerKind::Unattached
                || (
                    self.peer_ingress_owner_kind == PeerIngressOwnerKind::SessionOwned
                    && self.peer_ingress_comms_runtime_id == Some(comms_runtime_id)
                    && self.peer_ingress_mob_id == None
                )
            }
            update {
                self.peer_ingress_owner_kind = PeerIngressOwnerKind::SessionOwned;
                self.peer_ingress_comms_runtime_id = Some(comms_runtime_id);
                self.peer_ingress_mob_id = None;
            }
            to Idle
        }

        // AttachMobIngress: valid from `Unattached`, `SessionOwned`, or as
        // an exact idempotent re-assertion of the same mob-owned runtime.
        // Mob provisioning is allowed to take over a session-owned drain
        // (the spec's promotion case).
        transition AttachMobIngress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AttachMobIngress { comms_runtime_id, mob_id }
            guard "session_registered" { self.session_id != None }
            guard "owner_allows_mob_attach" {
                self.peer_ingress_owner_kind == PeerIngressOwnerKind::Unattached
                || self.peer_ingress_owner_kind == PeerIngressOwnerKind::SessionOwned
                || (
                    self.peer_ingress_owner_kind == PeerIngressOwnerKind::MobOwned
                    && self.peer_ingress_comms_runtime_id == Some(comms_runtime_id)
                    && self.peer_ingress_mob_id == Some(mob_id)
                )
            }
            update {
                self.peer_ingress_owner_kind = PeerIngressOwnerKind::MobOwned;
                self.peer_ingress_comms_runtime_id = Some(comms_runtime_id);
                self.peer_ingress_mob_id = Some(mob_id);
            }
            to Idle
        }

        // DetachIngress: clear any active ownership back to `Unattached`;
        // exact no-op detach is accepted as idempotence by the DSL itself.
        transition DetachIngress {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DetachIngress
            guard "session_registered" { self.session_id != None }
            update {
                self.peer_ingress_owner_kind = PeerIngressOwnerKind::Unattached;
                self.peer_ingress_comms_runtime_id = None;
                self.peer_ingress_mob_id = None;
            }
            to Idle
        }

        // =====================================================================
        // Supervisor-bridge authorization (Wave 3 D Row 21)
        // =====================================================================

        // BindSupervisor: only valid from `Unbound`. The shell-side
        // bootstrap gate (`validate_bind_request`) validates
        // sender-authentication and bootstrap token before firing this
        // input; the DSL owns the transition that flips the kind to
        // `Bound` and records the canonical identity + epoch.
        transition BindSupervisor {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input BindSupervisor { name, peer_id, address, epoch }
            guard "supervisor_unbound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Unbound
            }
            update {
                self.supervisor_binding_kind = SupervisorBindingKind::Bound;
                self.supervisor_bound_name = Some(name);
                self.supervisor_bound_peer_id = Some(peer_id);
                self.supervisor_bound_address = Some(address);
                self.supervisor_bound_epoch = Some(epoch);
            }
            to Idle
            emit PublishSupervisorTrustEdge {
                peer_id: self.supervisor_bound_peer_id.get("value"),
                name: self.supervisor_bound_name.get("value"),
                address: self.supervisor_bound_address.get("value"),
                signing_public_key: None,
                epoch: self.supervisor_bound_epoch.get("value")
            }
        }

        // AuthorizeSupervisor: only valid from `Bound`. Rotates the
        // current binding to a new supervisor + epoch. The shell-side
        // gate (`validate_authorize_supervisor_request`) enforces that
        // the rotation request is authenticated by the *current*
        // supervisor before firing this input.
        transition AuthorizeSupervisor {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AuthorizeSupervisor { name, peer_id, address, epoch }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            update {
                self.supervisor_bound_name = Some(name);
                self.supervisor_bound_peer_id = Some(peer_id);
                self.supervisor_bound_address = Some(address);
                self.supervisor_bound_epoch = Some(epoch);
            }
            to Idle
            emit PublishSupervisorTrustEdge {
                peer_id: self.supervisor_bound_peer_id.get("value"),
                name: self.supervisor_bound_name.get("value"),
                address: self.supervisor_bound_address.get("value"),
                signing_public_key: None,
                epoch: self.supervisor_bound_epoch.get("value")
            }
        }

        // RevokeSupervisor: only valid from `Bound`. The epoch and
        // peer_id must match the current binding exactly — a stale
        // revoke for a superseded supervisor/epoch cannot tear down a
        // freshly rotated binding.
        transition RevokeSupervisor {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RevokeSupervisor { peer_id, epoch }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            guard "peer_id_matches_current" {
                self.supervisor_bound_peer_id == Some(peer_id)
            }
            guard "epoch_matches_current" {
                self.supervisor_bound_epoch == Some(epoch)
            }
            update {
                self.supervisor_binding_kind = SupervisorBindingKind::Unbound;
                self.supervisor_bound_name = None;
                self.supervisor_bound_peer_id = None;
                self.supervisor_bound_address = None;
                self.supervisor_bound_epoch = None;
            }
            to Idle
            emit RevokeSupervisorTrustEdge { peer_id: peer_id, epoch: epoch }
        }

        // Supervisor-trust-edge feedback transitions (C-F2 / wave-d D-d).
        //
        // Each transition is phase-preserving and guards on both
        // `peer_id` and `epoch` matching the current binding. An ack for
        // epoch `N - 1` arriving after a rotation advanced the binding
        // to epoch `N` is rejected by the DSL — the outstanding
        // obligation stays open and the stale ack cannot satisfy it.
        transition SupervisorTrustEdgePublished {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SupervisorTrustEdgePublished { peer_id, epoch }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            guard "peer_id_matches_current" {
                self.supervisor_bound_peer_id == Some(peer_id)
            }
            guard "epoch_matches_current" {
                self.supervisor_bound_epoch == Some(epoch)
            }
            update {}
            to Idle
        }

        transition SupervisorTrustEdgePublishFailed {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SupervisorTrustEdgePublishFailed { peer_id, epoch, reason }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            guard "peer_id_matches_current" {
                self.supervisor_bound_peer_id == Some(peer_id)
            }
            guard "epoch_matches_current" {
                self.supervisor_bound_epoch == Some(epoch)
            }
            update {}
            to Idle
        }

        transition SupervisorTrustEdgeRevoked {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SupervisorTrustEdgeRevoked { peer_id, epoch }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            guard "peer_id_matches_current" {
                self.supervisor_bound_peer_id == Some(peer_id)
            }
            guard "epoch_matches_current" {
                self.supervisor_bound_epoch == Some(epoch)
            }
            update {}
            to Idle
        }

        transition SupervisorTrustEdgeRevokeFailed {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SupervisorTrustEdgeRevokeFailed { peer_id, epoch, reason }
            guard "supervisor_bound" {
                self.supervisor_binding_kind == SupervisorBindingKind::Bound
            }
            guard "peer_id_matches_current" {
                self.supervisor_bound_peer_id == Some(peer_id)
            }
            guard "epoch_matches_current" {
                self.supervisor_bound_epoch == Some(epoch)
            }
            update {}
            to Idle
        }

        // =====================================================================
        // Track-B (R5): peer-projection transitions.
        //
        // Valid in active session phases (`Idle`, `Attached`,
        // `Running`). See catalog DSL for the full rationale and
        // design notes. The `to Idle` clause is filler syntax —
        // `per_phase` expansion overrides `to_phase` to the current
        // phase (self-loop); the parser requires `to <Phase>` to be
        // present.
        // =====================================================================

        transition PublishLocalEndpoint {
            per_phase [Idle, Attached, Running]
            on input PublishLocalEndpoint { endpoint }
            update {
                self.local_endpoint = Some(endpoint);
            }
            to Idle
            emit LocalEndpointChanged { endpoint: Some(endpoint) }
        }

        transition ClearLocalEndpoint {
            per_phase [Idle, Attached, Running]
            on input ClearLocalEndpoint
            guard "local_endpoint_present" { self.local_endpoint != None }
            update {
                self.local_endpoint = None;
            }
            to Idle
            emit LocalEndpointChanged { endpoint: None }
        }

        transition AddDirectPeerEndpoint {
            per_phase [Idle, Attached, Running]
            on input AddDirectPeerEndpoint { endpoint }
            guard "endpoint_not_already_direct" { self.direct_peer_endpoints.contains(endpoint) == false }
            update {
                self.direct_peer_endpoints.insert(endpoint);
                self.peer_projection_epoch += 1;
            }
            to Idle
            emit PeerProjectionChanged { peer_projection_epoch: self.peer_projection_epoch }
            emit CommsTrustReconcileRequested { peer_projection_epoch: self.peer_projection_epoch }
        }

        transition RemoveDirectPeerEndpoint {
            per_phase [Idle, Attached, Running]
            on input RemoveDirectPeerEndpoint { endpoint }
            guard "endpoint_present_in_direct" { self.direct_peer_endpoints.contains(endpoint) == true }
            update {
                self.direct_peer_endpoints.remove(endpoint);
                self.peer_projection_epoch += 1;
            }
            to Idle
            emit PeerProjectionChanged { peer_projection_epoch: self.peer_projection_epoch }
            emit CommsTrustReconcileRequested { peer_projection_epoch: self.peer_projection_epoch }
        }

        transition ApplyMobPeerOverlay {
            per_phase [Idle, Attached, Running]
            on input ApplyMobPeerOverlay { epoch, endpoints }
            guard "stale_overlay_epoch" { epoch > self.mob_overlay_epoch }
            update {
                self.mob_overlay_peer_endpoints = endpoints;
                self.mob_overlay_epoch = epoch;
                self.peer_projection_epoch += 1;
            }
            to Idle
            emit PeerProjectionChanged { peer_projection_epoch: self.peer_projection_epoch }
            emit CommsTrustReconcileRequested { peer_projection_epoch: self.peer_projection_epoch }
        }
    }
}
    };
}

crate::meerkat_catalog_machine_dsl!("self", "catalog::dsl::meerkat_machine");
