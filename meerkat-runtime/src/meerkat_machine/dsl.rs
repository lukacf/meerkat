//! MeerkatMachine DSL definition with real bridging types.
use meerkat_machine_dsl::machine;
use meerkat_machine_schema::catalog::dsl::OptionValueExt;

// ---------------------------------------------------------------------------
// Bridging types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionId {
    pub fn from_domain(id: &meerkat_core::types::SessionId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AgentRuntimeId(pub String);

impl<T: Into<String>> From<T> for AgentRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl AgentRuntimeId {
    pub fn from_domain(id: &crate::identifiers::LogicalRuntimeId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl FenceToken {
    pub fn from_domain(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl Generation {
    pub fn from_domain(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RunId(pub String);

impl<T: Into<String>> From<T> for RunId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl RunId {
    pub fn from_domain(id: &meerkat_core::lifecycle::RunId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct InputId(pub String);

impl<T: Into<String>> From<T> for InputId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl InputId {
    pub fn from_domain(id: &meerkat_core::lifecycle::InputId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorkId(pub String);

impl<T: Into<String>> From<T> for WorkId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl WorkId {
    pub fn from_domain(id: &meerkat_core::lifecycle::InputId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OperationId(pub String);

impl<T: Into<String>> From<T> for OperationId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl OperationId {
    pub fn from_domain(id: &meerkat_core::ops::OperationId) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "\"unknown\"".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WaitRequestId(pub String);

impl<T: Into<String>> From<T> for WaitRequestId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl WaitRequestId {
    pub fn from_domain(id: &meerkat_core::lifecycle::WaitRequestId) -> Self {
        Self(id.to_string())
    }
}

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

impl From<meerkat_core::ops_lifecycle::OperationKind> for OperationKind {
    fn from(kind: meerkat_core::ops_lifecycle::OperationKind) -> Self {
        match kind {
            meerkat_core::ops_lifecycle::OperationKind::MobMemberChild => Self::MobMemberChild,
            meerkat_core::ops_lifecycle::OperationKind::BackgroundToolOp => Self::BackgroundToolOp,
        }
    }
}

impl From<OperationKind> for meerkat_core::ops_lifecycle::OperationKind {
    fn from(kind: OperationKind) -> Self {
        match kind {
            OperationKind::MobMemberChild => Self::MobMemberChild,
            OperationKind::BackgroundToolOp => Self::BackgroundToolOp,
        }
    }
}

impl OperationKind {
    pub fn from_domain(kind: &meerkat_core::ops_lifecycle::OperationKind) -> Self {
        Self::from(*kind)
    }
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

impl From<meerkat_core::provider::Provider> for Provider {
    fn from(p: meerkat_core::provider::Provider) -> Self {
        match p {
            meerkat_core::provider::Provider::Anthropic => Self::Anthropic,
            meerkat_core::provider::Provider::OpenAI => Self::OpenAI,
            meerkat_core::provider::Provider::Gemini => Self::Gemini,
            meerkat_core::provider::Provider::SelfHosted => Self::SelfHosted,
            meerkat_core::provider::Provider::Other => Self::Other,
        }
    }
}

impl From<Provider> for meerkat_core::provider::Provider {
    fn from(p: Provider) -> Self {
        match p {
            Provider::Anthropic => Self::Anthropic,
            Provider::OpenAI => Self::OpenAI,
            Provider::Gemini => Self::Gemini,
            Provider::SelfHosted => Self::SelfHosted,
            Provider::Other => Self::Other,
        }
    }
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

impl From<&meerkat_core::ConnectionRef> for ConnectionRef {
    fn from(r: &meerkat_core::ConnectionRef) -> Self {
        Self {
            realm_id: r.realm.as_str().to_owned(),
            binding_id: r.binding.as_str().to_owned(),
            profile_id: r.profile.as_ref().map(|p| p.as_str().to_owned()),
        }
    }
}

/// Fallible conversion — DSL-layer flat strings may be slug-invalid
/// (the DSL mirror intentionally accepts opaque strings to survive
/// deserialization drift across schema versions), so lifting back to
/// the typed-atom domain form may reject.
impl TryFrom<ConnectionRef> for meerkat_core::ConnectionRef {
    type Error = meerkat_core::IdentityError;

    fn try_from(r: ConnectionRef) -> Result<Self, Self::Error> {
        Ok(Self {
            realm: meerkat_core::RealmId::parse(&r.realm_id)?,
            binding: meerkat_core::BindingId::parse(&r.binding_id)?,
            profile: r
                .profile_id
                .as_deref()
                .map(meerkat_core::ProfileId::parse)
                .transpose()?,
        })
    }
}

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

impl SessionLlmIdentity {
    pub fn from_domain(id: &meerkat_core::SessionLlmIdentity) -> Self {
        Self {
            model: id.model.clone(),
            provider: Provider::from(id.provider),
            self_hosted_server_id: id.self_hosted_server_id.clone(),
            provider_params_repr: id
                .provider_params
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_default()),
            connection_ref: id.connection_ref.as_ref().map(ConnectionRef::from),
        }
    }
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

impl SessionToolVisibilityState {
    pub fn from_domain(id: &meerkat_core::SessionToolVisibilityState) -> Self {
        Self {
            capability_base_filter: ToolFilter::from(&id.capability_base_filter),
            inherited_base_filter: ToolFilter::from(&id.inherited_base_filter),
            active_filter: ToolFilter::from(&id.active_filter),
            staged_filter: ToolFilter::from(&id.staged_filter),
            active_requested_deferred_names: id.active_requested_deferred_names.clone(),
            staged_requested_deferred_names: id.staged_requested_deferred_names.clone(),
            active_revision: id.active_revision,
            staged_revision: id.staged_revision,
            requested_witnesses: id
                .requested_witnesses
                .iter()
                .map(|(k, w)| (k.clone(), ToolVisibilityWitness::from(w)))
                .collect(),
            filter_witnesses: id
                .filter_witnesses
                .iter()
                .map(|(k, w)| (k.clone(), ToolVisibilityWitness::from(w)))
                .collect(),
        }
    }
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

impl From<&crate::meerkat_machine_types::SessionLlmCapabilitySurface>
    for SessionLlmCapabilitySurface
{
    fn from(s: &crate::meerkat_machine_types::SessionLlmCapabilitySurface) -> Self {
        Self {
            supports_temperature: s.supports_temperature,
            supports_thinking: s.supports_thinking,
            supports_reasoning: s.supports_reasoning,
            inline_video: s.inline_video,
            vision: s.vision,
            image_tool_results: s.image_tool_results,
            supports_web_search: s.supports_web_search,
            realtime: s.realtime,
            call_timeout_secs: s.call_timeout_secs,
        }
    }
}

impl SessionLlmCapabilitySurface {
    pub fn from_domain(id: &crate::meerkat_machine_types::SessionLlmCapabilitySurface) -> Self {
        Self::from(id)
    }
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

impl From<crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus>
    for SessionLlmCapabilitySurfaceStatus
{
    fn from(status: crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus) -> Self {
        match status {
            crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus::Resolved => {
                Self::Resolved
            }
            crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus::Unresolved => {
                Self::Unresolved
            }
        }
    }
}

impl From<SessionLlmCapabilitySurfaceStatus>
    for crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus
{
    fn from(status: SessionLlmCapabilitySurfaceStatus) -> Self {
        match status {
            SessionLlmCapabilitySurfaceStatus::Resolved => Self::Resolved,
            SessionLlmCapabilitySurfaceStatus::Unresolved => Self::Unresolved,
        }
    }
}

impl SessionLlmCapabilitySurfaceStatus {
    pub fn from_domain(
        id: &crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus,
    ) -> Self {
        Self::from(*id)
    }
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

impl SessionToolVisibilityDelta {
    pub fn from_domain(id: &crate::meerkat_machine_types::SessionToolVisibilityDelta) -> Self {
        Self {
            previous_capability_base_filter: ToolFilter::from(&id.previous_capability_base_filter),
            current_capability_base_filter: ToolFilter::from(&id.current_capability_base_filter),
            committed_visible_set_changed: id.committed_visible_set_changed,
            revision_bumped: id.revision_bumped,
        }
    }
}

/// Typed mirror of [`meerkat_core::ToolFilter`] — closed 3-variant
/// discriminant with a `BTreeSet<String>` name payload for
/// `Allow`/`Deny` so the value is `Ord + Hash` and deterministic across
/// iteration, matching the R3 `InputAbandonReason::MaxAttemptsExhausted {
/// attempts }` pattern of carrying the discriminant's companion data in a
/// field with stable ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ToolFilter {
    #[default]
    All,
    Allow(std::collections::BTreeSet<String>),
    Deny(std::collections::BTreeSet<String>),
}

impl From<&meerkat_core::ToolFilter> for ToolFilter {
    fn from(f: &meerkat_core::ToolFilter) -> Self {
        match f {
            meerkat_core::ToolFilter::All => Self::All,
            meerkat_core::ToolFilter::Allow(names) => {
                Self::Allow(names.iter().map(|name| name.as_str().to_string()).collect())
            }
            meerkat_core::ToolFilter::Deny(names) => {
                Self::Deny(names.iter().map(|name| name.as_str().to_string()).collect())
            }
        }
    }
}

impl From<ToolFilter> for meerkat_core::ToolFilter {
    fn from(f: ToolFilter) -> Self {
        match f {
            ToolFilter::All => Self::All,
            ToolFilter::Allow(names) => Self::Allow(names.into_iter().collect()),
            ToolFilter::Deny(names) => Self::Deny(names.into_iter().collect()),
        }
    }
}

impl ToolFilter {
    pub fn from_domain(id: &meerkat_core::ToolFilter) -> Self {
        Self::from(id)
    }
}

/// Typed mirror of [`meerkat_core::types::ToolSourceKind`] — closed
/// 10-variant discriminant for tool provenance classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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

impl From<&meerkat_core::types::ToolSourceKind> for ToolSourceKind {
    fn from(k: &meerkat_core::types::ToolSourceKind) -> Self {
        match k {
            meerkat_core::types::ToolSourceKind::Builtin => Self::Builtin,
            meerkat_core::types::ToolSourceKind::Shell => Self::Shell,
            meerkat_core::types::ToolSourceKind::Comms => Self::Comms,
            meerkat_core::types::ToolSourceKind::Memory => Self::Memory,
            meerkat_core::types::ToolSourceKind::Schedule => Self::Schedule,
            meerkat_core::types::ToolSourceKind::Mob => Self::Mob,
            meerkat_core::types::ToolSourceKind::MobTasks => Self::MobTasks,
            meerkat_core::types::ToolSourceKind::Callback => Self::Callback,
            meerkat_core::types::ToolSourceKind::Mcp => Self::Mcp,
            meerkat_core::types::ToolSourceKind::RustBundle => Self::RustBundle,
        }
    }
}

/// Typed mirror of [`meerkat_core::types::ToolProvenance`] — structural
/// projection carried inside [`ToolVisibilityWitness`], using the typed
/// `ToolSourceKind` discriminant mirror.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ToolProvenance {
    pub kind: ToolSourceKind,
    pub source_id: String,
}

impl From<&meerkat_core::types::ToolProvenance> for ToolProvenance {
    fn from(p: &meerkat_core::types::ToolProvenance) -> Self {
        Self {
            kind: ToolSourceKind::from(&p.kind),
            source_id: p.source_id.to_string(),
        }
    }
}

/// Typed mirror of [`meerkat_core::ToolVisibilityWitness`] — structural
/// projection of the two optional witness fields.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ToolVisibilityWitness {
    pub stable_owner_key: Option<String>,
    pub last_seen_provenance: Option<ToolProvenance>,
}

impl From<&meerkat_core::ToolVisibilityWitness> for ToolVisibilityWitness {
    fn from(w: &meerkat_core::ToolVisibilityWitness) -> Self {
        Self {
            stable_owner_key: w.stable_owner_key.clone(),
            last_seen_provenance: w.last_seen_provenance.as_ref().map(ToolProvenance::from),
        }
    }
}

impl ToolVisibilityWitness {
    pub fn from_domain(id: &meerkat_core::ToolVisibilityWitness) -> Self {
        Self::from(id)
    }

    fn has_identity_witness(&self) -> bool {
        self.stable_owner_key.is_some() || self.last_seen_provenance.is_some()
    }

    fn len(&self) -> u64 {
        u64::from(self.has_identity_witness())
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
/// public status projection live in the MeerkatMachine state.
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

impl<T: Into<String>> From<T> for McpServerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging wrapper mapping [`meerkat_core::PeerCorrelationId`] into the DSL
/// macro's type system. Keyed map values for `pending_peer_requests` and
/// `inbound_peer_requests`; carried on every W1-A peer-lifecycle input and
/// effect.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PeerCorrelationId(pub String);

impl From<meerkat_core::PeerCorrelationId> for PeerCorrelationId {
    fn from(id: meerkat_core::PeerCorrelationId) -> Self {
        Self(id.0.to_string())
    }
}

impl From<uuid::Uuid> for PeerCorrelationId {
    fn from(id: uuid::Uuid) -> Self {
        Self(id.to_string())
    }
}

impl From<String> for PeerCorrelationId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PeerCorrelationId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

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

impl From<meerkat_core::OutboundPeerRequestState> for OutboundPeerRequestState {
    fn from(s: meerkat_core::OutboundPeerRequestState) -> Self {
        match s {
            meerkat_core::OutboundPeerRequestState::Sent => Self::Sent,
            meerkat_core::OutboundPeerRequestState::AcceptedProgress => Self::AcceptedProgress,
            meerkat_core::OutboundPeerRequestState::Completed => Self::Completed,
            meerkat_core::OutboundPeerRequestState::Failed => Self::Failed,
            meerkat_core::OutboundPeerRequestState::TimedOut => Self::TimedOut,
            // core `#[non_exhaustive]` guard: new variants added there
            // without a catalog mirror fall back to `Sent`. Detect at
            // codegen time via the parity test, not at runtime.
            _ => Self::Sent,
        }
    }
}

impl From<OutboundPeerRequestState> for meerkat_core::OutboundPeerRequestState {
    fn from(s: OutboundPeerRequestState) -> Self {
        match s {
            OutboundPeerRequestState::Sent => Self::Sent,
            OutboundPeerRequestState::AcceptedProgress => Self::AcceptedProgress,
            OutboundPeerRequestState::Completed => Self::Completed,
            OutboundPeerRequestState::Failed => Self::Failed,
            OutboundPeerRequestState::TimedOut => Self::TimedOut,
        }
    }
}

/// Typed inbound peer-request state, mirroring
/// [`meerkat_core::InboundPeerRequestState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InboundPeerRequestState {
    #[default]
    Received,
    Replied,
}

impl From<meerkat_core::InboundPeerRequestState> for InboundPeerRequestState {
    fn from(s: meerkat_core::InboundPeerRequestState) -> Self {
        match s {
            meerkat_core::InboundPeerRequestState::Received => Self::Received,
            meerkat_core::InboundPeerRequestState::Replied => Self::Replied,
            _ => Self::Received,
        }
    }
}

impl From<InboundPeerRequestState> for meerkat_core::InboundPeerRequestState {
    fn from(s: InboundPeerRequestState) -> Self {
        match s {
            InboundPeerRequestState::Received => Self::Received,
            InboundPeerRequestState::Replied => Self::Replied,
        }
    }
}

/// Typed terminal disposition carried on `PeerResponseTerminalArrived`.
/// Mirror of [`meerkat_core::handles::PeerTerminalDisposition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerTerminalDisposition {
    #[default]
    Completed,
    Failed,
}

impl From<meerkat_core::handles::PeerTerminalDisposition> for PeerTerminalDisposition {
    fn from(d: meerkat_core::handles::PeerTerminalDisposition) -> Self {
        match d {
            meerkat_core::handles::PeerTerminalDisposition::Completed => Self::Completed,
            meerkat_core::handles::PeerTerminalDisposition::Failed => Self::Failed,
            _ => Self::Failed,
        }
    }
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

impl From<meerkat_core::InteractionStreamState> for InteractionStreamState {
    fn from(s: meerkat_core::InteractionStreamState) -> Self {
        match s {
            meerkat_core::InteractionStreamState::Reserved => Self::Reserved,
            meerkat_core::InteractionStreamState::Attached => Self::Attached,
            meerkat_core::InteractionStreamState::Completed => Self::Completed,
            meerkat_core::InteractionStreamState::Expired => Self::Expired,
            meerkat_core::InteractionStreamState::ClosedEarly => Self::ClosedEarly,
            _ => Self::Reserved,
        }
    }
}

impl From<InteractionStreamState> for meerkat_core::InteractionStreamState {
    fn from(s: InteractionStreamState) -> Self {
        match s {
            InteractionStreamState::Reserved => Self::Reserved,
            InteractionStreamState::Attached => Self::Attached,
            InteractionStreamState::Completed => Self::Completed,
            InteractionStreamState::Expired => Self::Expired,
            InteractionStreamState::ClosedEarly => Self::ClosedEarly,
        }
    }
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

impl<T: Into<String>> From<T> for CommsRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl CommsRuntimeId {
    /// Derive a stable id from an `Arc<dyn CommsRuntime>`'s pointer address.
    ///
    /// Two `Arc` instances with the same pointee produce the same id; two
    /// distinct `Arc` instances produce distinct ids even if their contents
    /// are equivalent. This is sufficient for detecting silent transport
    /// swaps at the DSL boundary.
    pub fn from_runtime(runtime: &std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>) -> Self {
        let ptr = std::sync::Arc::as_ptr(runtime).cast::<()>() as usize;
        Self(format!("comms-runtime-0x{ptr:x}"))
    }
}

/// Mob instance identifier for peer-ingress ownership (W2-G / issue #264).
///
/// Bridging newtype mirroring `meerkat_mob::ids::MobId`. The DSL layer keeps
/// this opaque because `meerkat-runtime` does not depend on `meerkat-mob`;
/// the shell stringifies the real `MobId` before firing `AttachMobIngress`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MobId(pub String);

impl<T: Into<String>> From<T> for MobId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

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

impl TurnPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::ApplyingPrimitive => "ApplyingPrimitive",
            Self::CallingLlm => "CallingLlm",
            Self::WaitingForOps => "WaitingForOps",
            Self::DrainingBoundary => "DrainingBoundary",
            Self::Extracting => "Extracting",
            Self::ErrorRecovery => "ErrorRecovery",
            Self::Cancelling => "Cancelling",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Typed registration substate. Closed set of literals previously assigned to
/// `registration_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RegistrationPhase {
    #[default]
    Queuing,
    Active,
}

impl RegistrationPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queuing => "Queuing",
            Self::Active => "Active",
        }
    }
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

impl DrainPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "Inactive",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::ExitedRespawnable => "ExitedRespawnable",
        }
    }
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

impl DrainMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timed => "Timed",
            Self::AttachedSession => "AttachedSession",
            Self::PersistentHost => "PersistentHost",
        }
    }
}

impl From<crate::meerkat_machine::CommsDrainMode> for DrainMode {
    fn from(mode: crate::meerkat_machine::CommsDrainMode) -> Self {
        match mode {
            crate::meerkat_machine::CommsDrainMode::Timed => Self::Timed,
            crate::meerkat_machine::CommsDrainMode::AttachedSession => Self::AttachedSession,
            crate::meerkat_machine::CommsDrainMode::PersistentHost => Self::PersistentHost,
        }
    }
}

/// Typed external-tool surface global phase. Closed set of literals previously
/// assigned to `surface_phase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SurfacePhase {
    #[default]
    Operating,
    Shutdown,
}

impl SurfacePhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operating => "Operating",
            Self::Shutdown => "Shutdown",
        }
    }
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

impl LiveTopologyPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Reconfiguring => "Reconfiguring",
            Self::Detached => "Detached",
            Self::HostIdentityApplied => "HostIdentityApplied",
            Self::HostVisibilityApplied => "HostVisibilityApplied",
        }
    }
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

impl InputPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Staged => "Staged",
            Self::Applied => "Applied",
            Self::AppliedPendingConsumption => "AppliedPendingConsumption",
            Self::Consumed => "Consumed",
            Self::Superseded => "Superseded",
            Self::Coalesced => "Coalesced",
            Self::Abandoned => "Abandoned",
        }
    }
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

impl InputTerminalKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Consumed => "Consumed",
            Self::Superseded => "Superseded",
            Self::Coalesced => "Coalesced",
            Self::Abandoned => "Abandoned",
        }
    }
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

impl SurfacePendingOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Add => "Add",
            Self::Reload => "Reload",
        }
    }
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

impl SurfaceStagedOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Add => "Add",
            Self::Remove => "Remove",
            Self::Reload => "Reload",
        }
    }
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

impl From<meerkat_core::turn_execution_authority::TurnPrimitiveKind> for TurnPrimitiveKind {
    fn from(kind: meerkat_core::turn_execution_authority::TurnPrimitiveKind) -> Self {
        match kind {
            meerkat_core::turn_execution_authority::TurnPrimitiveKind::None => Self::None,
            meerkat_core::turn_execution_authority::TurnPrimitiveKind::ConversationTurn => {
                Self::ConversationTurn
            }
            meerkat_core::turn_execution_authority::TurnPrimitiveKind::ImmediateAppend => {
                Self::ImmediateAppend
            }
            meerkat_core::turn_execution_authority::TurnPrimitiveKind::ImmediateContextAppend => {
                Self::ImmediateContextAppend
            }
        }
    }
}

impl From<TurnPrimitiveKind> for meerkat_core::turn_execution_authority::TurnPrimitiveKind {
    fn from(kind: TurnPrimitiveKind) -> Self {
        match kind {
            TurnPrimitiveKind::None => Self::None,
            TurnPrimitiveKind::ConversationTurn => Self::ConversationTurn,
            TurnPrimitiveKind::ImmediateAppend => Self::ImmediateAppend,
            TurnPrimitiveKind::ImmediateContextAppend => Self::ImmediateContextAppend,
        }
    }
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

impl From<meerkat_core::turn_execution_authority::TurnTerminalOutcome> for TurnTerminalOutcome {
    fn from(outcome: meerkat_core::turn_execution_authority::TurnTerminalOutcome) -> Self {
        match outcome {
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::None => Self::None,
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::Completed => {
                Self::Completed
            }
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::Failed => Self::Failed,
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::Cancelled => {
                Self::Cancelled
            }
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::BudgetExhausted => {
                Self::BudgetExhausted
            }
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::TimeBudgetExceeded => {
                Self::TimeBudgetExceeded
            }
            meerkat_core::turn_execution_authority::TurnTerminalOutcome::StructuredOutputValidationFailed => {
                Self::StructuredOutputValidationFailed
            }
        }
    }
}

impl From<TurnTerminalOutcome> for meerkat_core::turn_execution_authority::TurnTerminalOutcome {
    fn from(outcome: TurnTerminalOutcome) -> Self {
        match outcome {
            TurnTerminalOutcome::None => Self::None,
            TurnTerminalOutcome::Completed => Self::Completed,
            TurnTerminalOutcome::Failed => Self::Failed,
            TurnTerminalOutcome::Cancelled => Self::Cancelled,
            TurnTerminalOutcome::BudgetExhausted => Self::BudgetExhausted,
            TurnTerminalOutcome::TimeBudgetExceeded => Self::TimeBudgetExceeded,
            TurnTerminalOutcome::StructuredOutputValidationFailed => {
                Self::StructuredOutputValidationFailed
            }
        }
    }
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
    ExecutorStopped,
    ExecutorControlFailed,
    ExecutorInternal,
}

impl From<meerkat_core::lifecycle::CoreApplyFailureCauseKind> for RuntimeApplyFailureCause {
    fn from(kind: meerkat_core::lifecycle::CoreApplyFailureCauseKind) -> Self {
        match kind {
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::PrimitiveRejected => {
                Self::PrimitiveRejected
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::RuntimeContextApply => {
                Self::RuntimeContextApply
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::RuntimeTurn => Self::RuntimeTurn,
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::ExecutorStopped => {
                Self::ExecutorStopped
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::ExecutorControlFailed => {
                Self::ExecutorControlFailed
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::ExecutorInternal => {
                Self::ExecutorInternal
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::Unknown => Self::Unknown,
            _ => Self::Unknown,
        }
    }
}

impl From<&meerkat_core::lifecycle::CoreApplyFailureCause> for RuntimeApplyFailureCause {
    fn from(cause: &meerkat_core::lifecycle::CoreApplyFailureCause) -> Self {
        Self::from(cause.kind)
    }
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

impl From<meerkat_core::retry::LlmRetryFailureKind> for LlmRetryFailureKind {
    fn from(kind: meerkat_core::retry::LlmRetryFailureKind) -> Self {
        match kind {
            meerkat_core::retry::LlmRetryFailureKind::RateLimited => Self::RateLimited,
            meerkat_core::retry::LlmRetryFailureKind::NetworkTimeout => Self::NetworkTimeout,
            meerkat_core::retry::LlmRetryFailureKind::CallTimeout => Self::CallTimeout,
            meerkat_core::retry::LlmRetryFailureKind::RetryableProviderError => {
                Self::RetryableProviderError
            }
        }
    }
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

impl From<meerkat_core::tool_scope::ExternalToolSurfaceBaseState> for ExternalToolSurfaceBaseState {
    fn from(state: meerkat_core::tool_scope::ExternalToolSurfaceBaseState) -> Self {
        match state {
            meerkat_core::tool_scope::ExternalToolSurfaceBaseState::Absent => Self::Absent,
            meerkat_core::tool_scope::ExternalToolSurfaceBaseState::Active => Self::Active,
            meerkat_core::tool_scope::ExternalToolSurfaceBaseState::Removing => Self::Removing,
            meerkat_core::tool_scope::ExternalToolSurfaceBaseState::Removed => Self::Removed,
        }
    }
}

impl From<ExternalToolSurfaceBaseState> for meerkat_core::tool_scope::ExternalToolSurfaceBaseState {
    fn from(state: ExternalToolSurfaceBaseState) -> Self {
        match state {
            ExternalToolSurfaceBaseState::Absent => Self::Absent,
            ExternalToolSurfaceBaseState::Active => Self::Active,
            ExternalToolSurfaceBaseState::Removing => Self::Removing,
            ExternalToolSurfaceBaseState::Removed => Self::Removed,
        }
    }
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

impl From<meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation>
    for ExternalToolSurfaceDeltaOperation
{
    fn from(op: meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation) -> Self {
        match op {
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation::None => Self::None,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation::Add => Self::Add,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation::Remove => Self::Remove,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation::Reload => Self::Reload,
        }
    }
}

impl From<ExternalToolSurfaceDeltaOperation>
    for meerkat_core::tool_scope::ExternalToolSurfaceDeltaOperation
{
    fn from(op: ExternalToolSurfaceDeltaOperation) -> Self {
        match op {
            ExternalToolSurfaceDeltaOperation::None => Self::None,
            ExternalToolSurfaceDeltaOperation::Add => Self::Add,
            ExternalToolSurfaceDeltaOperation::Remove => Self::Remove,
            ExternalToolSurfaceDeltaOperation::Reload => Self::Reload,
        }
    }
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

impl From<meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase>
    for ExternalToolSurfaceDeltaPhase
{
    fn from(phase: meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase) -> Self {
        match phase {
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase::None => Self::None,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase::Pending => Self::Pending,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase::Applied => Self::Applied,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase::Draining => Self::Draining,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase::Failed => Self::Failed,
            meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase::Forced => Self::Forced,
        }
    }
}

impl From<ExternalToolSurfaceDeltaPhase>
    for meerkat_core::tool_scope::ExternalToolSurfaceDeltaPhase
{
    fn from(phase: ExternalToolSurfaceDeltaPhase) -> Self {
        match phase {
            ExternalToolSurfaceDeltaPhase::None => Self::None,
            ExternalToolSurfaceDeltaPhase::Pending => Self::Pending,
            ExternalToolSurfaceDeltaPhase::Applied => Self::Applied,
            ExternalToolSurfaceDeltaPhase::Draining => Self::Draining,
            ExternalToolSurfaceDeltaPhase::Failed => Self::Failed,
            ExternalToolSurfaceDeltaPhase::Forced => Self::Forced,
        }
    }
}

/// Typed failure cause for an external tool surface. Closed mirror of
/// [`meerkat_core::tool_scope::ExternalToolSurfaceFailureCause`] so pending
/// failure and call-rejection causes cross the DSL as data, not string codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ExternalToolSurfaceFailureCause {
    #[default]
    PendingFailed,
    SurfaceDraining,
    SurfaceUnavailable,
}

impl From<meerkat_core::tool_scope::ExternalToolSurfaceFailureCause>
    for ExternalToolSurfaceFailureCause
{
    fn from(cause: meerkat_core::tool_scope::ExternalToolSurfaceFailureCause) -> Self {
        match cause {
            meerkat_core::tool_scope::ExternalToolSurfaceFailureCause::PendingFailed => {
                Self::PendingFailed
            }
            meerkat_core::tool_scope::ExternalToolSurfaceFailureCause::SurfaceDraining => {
                Self::SurfaceDraining
            }
            meerkat_core::tool_scope::ExternalToolSurfaceFailureCause::SurfaceUnavailable => {
                Self::SurfaceUnavailable
            }
        }
    }
}

impl From<ExternalToolSurfaceFailureCause>
    for meerkat_core::tool_scope::ExternalToolSurfaceFailureCause
{
    fn from(cause: ExternalToolSurfaceFailureCause) -> Self {
        match cause {
            ExternalToolSurfaceFailureCause::PendingFailed => Self::PendingFailed,
            ExternalToolSurfaceFailureCause::SurfaceDraining => Self::SurfaceDraining,
            ExternalToolSurfaceFailureCause::SurfaceUnavailable => Self::SurfaceUnavailable,
        }
    }
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

impl From<meerkat_core::handles::DrainExitReason> for DrainExitReason {
    fn from(reason: meerkat_core::handles::DrainExitReason) -> Self {
        match reason {
            meerkat_core::handles::DrainExitReason::IdleTimeout => Self::IdleTimeout,
            meerkat_core::handles::DrainExitReason::Dismissed => Self::Dismissed,
            meerkat_core::handles::DrainExitReason::Failed => Self::Failed,
            meerkat_core::handles::DrainExitReason::Aborted => Self::Aborted,
            meerkat_core::handles::DrainExitReason::SessionShutdown => Self::SessionShutdown,
        }
    }
}

impl From<DrainExitReason> for meerkat_core::handles::DrainExitReason {
    fn from(reason: DrainExitReason) -> Self {
        match reason {
            DrainExitReason::IdleTimeout => Self::IdleTimeout,
            DrainExitReason::Dismissed => Self::Dismissed,
            DrainExitReason::Failed => Self::Failed,
            DrainExitReason::Aborted => Self::Aborted,
            DrainExitReason::SessionShutdown => Self::SessionShutdown,
        }
    }
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

impl From<meerkat_core::comms::InputSource> for WorkOrigin {
    fn from(src: meerkat_core::comms::InputSource) -> Self {
        match src {
            // Transport-originated inputs are `External` work-lane: they
            // entered the runtime via a non-mob transport (TCP/UDS/stdin/
            // webhook/RPC). Mob-originated work fires the DSL directly
            // with `External`/`Internal` instead of going through the
            // session-admission handle.
            meerkat_core::comms::InputSource::Tcp
            | meerkat_core::comms::InputSource::Uds
            | meerkat_core::comms::InputSource::Stdin
            | meerkat_core::comms::InputSource::Webhook
            | meerkat_core::comms::InputSource::Rpc => Self::External,
        }
    }
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

impl From<meerkat_core::ops_lifecycle::OperationStatus> for OperationStatus {
    fn from(status: meerkat_core::ops_lifecycle::OperationStatus) -> Self {
        match status {
            meerkat_core::ops_lifecycle::OperationStatus::Absent => Self::Absent,
            meerkat_core::ops_lifecycle::OperationStatus::Provisioning => Self::Provisioning,
            meerkat_core::ops_lifecycle::OperationStatus::Running => Self::Running,
            meerkat_core::ops_lifecycle::OperationStatus::Retiring => Self::Retiring,
            meerkat_core::ops_lifecycle::OperationStatus::Completed => Self::Completed,
            meerkat_core::ops_lifecycle::OperationStatus::Failed => Self::Failed,
            meerkat_core::ops_lifecycle::OperationStatus::Aborted => Self::Aborted,
            meerkat_core::ops_lifecycle::OperationStatus::Cancelled => Self::Cancelled,
            meerkat_core::ops_lifecycle::OperationStatus::Retired => Self::Retired,
            meerkat_core::ops_lifecycle::OperationStatus::Terminated => Self::Terminated,
        }
    }
}

impl From<OperationStatus> for meerkat_core::ops_lifecycle::OperationStatus {
    fn from(status: OperationStatus) -> Self {
        match status {
            OperationStatus::Absent => Self::Absent,
            OperationStatus::Provisioning => Self::Provisioning,
            OperationStatus::Running => Self::Running,
            OperationStatus::Retiring => Self::Retiring,
            OperationStatus::Completed => Self::Completed,
            OperationStatus::Failed => Self::Failed,
            OperationStatus::Aborted => Self::Aborted,
            OperationStatus::Cancelled => Self::Cancelled,
            OperationStatus::Retired => Self::Retired,
            OperationStatus::Terminated => Self::Terminated,
        }
    }
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

impl From<&crate::input_state::InputAbandonReason> for InputAbandonReason {
    fn from(reason: &crate::input_state::InputAbandonReason) -> Self {
        match reason {
            crate::input_state::InputAbandonReason::Retired => Self::Retired,
            crate::input_state::InputAbandonReason::Reset => Self::Reset,
            crate::input_state::InputAbandonReason::Stopped => Self::Stopped,
            crate::input_state::InputAbandonReason::Destroyed => Self::Destroyed,
            crate::input_state::InputAbandonReason::Cancelled => Self::Cancelled,
            crate::input_state::InputAbandonReason::MaxAttemptsExhausted { .. } => {
                Self::MaxAttemptsExhausted
            }
        }
    }
}

impl InputAbandonReason {
    /// Stable lowercase label for event wire formats. Mirrors the
    /// snake-case serde representation of the domain enum for consistency
    /// with existing consumers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retired => "retired",
            Self::Reset => "reset",
            Self::Stopped => "stopped",
            Self::Destroyed => "destroyed",
            Self::Cancelled => "cancelled",
            Self::MaxAttemptsExhausted => "max_attempts_exhausted",
        }
    }
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

impl From<crate::HandlingMode> for InputLane {
    fn from(mode: crate::HandlingMode) -> Self {
        match mode {
            crate::HandlingMode::Queue => Self::Queue,
            crate::HandlingMode::Steer => Self::Steer,
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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

// MeerkatMachine production body is catalog-owned. Keep bridge/runtime mechanics
// outside this macro invocation; canonical semantics live in the catalog DSL.
meerkat_machine_schema::meerkat_catalog_machine_dsl!("meerkat-runtime", "meerkat_machine::dsl");

// =====================================================================
