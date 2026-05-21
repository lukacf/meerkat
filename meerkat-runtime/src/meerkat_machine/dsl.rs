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

/// Typed mirror of [`meerkat_core::AuthBindingRef`] — structural string
/// projection carrying the flat forms of `realm` / `binding` / `profile`
/// with bidirectional `From`.
///
/// The DSL layer keeps string fields because this mirror is the
/// DSL-layer identity carrier (used inside runtime-owned guards /
/// transitions where slug validation has already happened at the
/// boundary). Domain-side `AuthBindingRef` carries the typed atoms
/// (`RealmId` / `BindingId` / `ProfileId`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AuthBindingRef {
    pub realm_id: String,
    pub binding_id: String,
    pub profile_id: Option<String>,
}

impl From<&meerkat_core::AuthBindingRef> for AuthBindingRef {
    fn from(r: &meerkat_core::AuthBindingRef) -> Self {
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
impl TryFrom<AuthBindingRef> for meerkat_core::AuthBindingRef {
    type Error = meerkat_core::IdentityError;

    fn try_from(r: AuthBindingRef) -> Result<Self, Self::Error> {
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
/// projection with typed `Provider` and `AuthBindingRef` mirrors. The
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
    pub auth_binding: Option<AuthBindingRef>,
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
            auth_binding: id.auth_binding.as_ref().map(AuthBindingRef::from),
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
    pub image_input: bool,
    pub image_tool_results: bool,
    pub supports_web_search: bool,
    pub image_generation: bool,
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
            image_input: s.image_input,
            image_tool_results: s.image_tool_results,
            supports_web_search: s.supports_web_search,
            image_generation: s.image_generation,
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
/// Closed discriminant for tool provenance classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ToolSourceKind {
    #[default]
    Builtin,
    Shell,
    Comms,
    Memory,
    Schedule,
    WorkGraph,
    Mob,
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
            meerkat_core::types::ToolSourceKind::WorkGraph => Self::WorkGraph,
            meerkat_core::types::ToolSourceKind::Mob => Self::Mob,
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

    fn len(&self) -> u64 {
        u64::from(self.last_seen_provenance.is_some())
    }
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
    #[allow(clippy::panic)]
    fn from(s: meerkat_core::OutboundPeerRequestState) -> Self {
        match s {
            meerkat_core::OutboundPeerRequestState::Sent => Self::Sent,
            meerkat_core::OutboundPeerRequestState::AcceptedProgress => Self::AcceptedProgress,
            meerkat_core::OutboundPeerRequestState::Completed => Self::Completed,
            meerkat_core::OutboundPeerRequestState::Failed => Self::Failed,
            meerkat_core::OutboundPeerRequestState::TimedOut => Self::TimedOut,
            _ => panic!(
                "unsupported OutboundPeerRequestState variant; update generated MeerkatMachine mirror"
            ),
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
    #[allow(clippy::panic)]
    fn from(s: meerkat_core::InboundPeerRequestState) -> Self {
        match s {
            meerkat_core::InboundPeerRequestState::Received => Self::Received,
            meerkat_core::InboundPeerRequestState::Replied => Self::Replied,
            _ => panic!(
                "unsupported InboundPeerRequestState variant; update generated MeerkatMachine mirror"
            ),
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
    #[allow(clippy::panic)]
    fn from(d: meerkat_core::handles::PeerTerminalDisposition) -> Self {
        match d {
            meerkat_core::handles::PeerTerminalDisposition::Completed => Self::Completed,
            meerkat_core::handles::PeerTerminalDisposition::Failed => Self::Failed,
            _ => panic!(
                "unsupported PeerTerminalDisposition variant; update generated MeerkatMachine mirror"
            ),
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
    #[allow(clippy::panic)]
    fn from(s: meerkat_core::InteractionStreamState) -> Self {
        match s {
            meerkat_core::InteractionStreamState::Reserved => Self::Reserved,
            meerkat_core::InteractionStreamState::Attached => Self::Attached,
            meerkat_core::InteractionStreamState::Completed => Self::Completed,
            meerkat_core::InteractionStreamState::Expired => Self::Expired,
            meerkat_core::InteractionStreamState::ClosedEarly => Self::ClosedEarly,
            _ => panic!(
                "unsupported InteractionStreamState variant; update generated MeerkatMachine mirror"
            ),
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

/// DSL-owned public peer-ingress authority phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressAuthorityPhaseClass {
    #[default]
    Absent,
    Received,
    Dropped,
    Delivered,
}

impl From<PeerIngressAuthorityPhaseClass> for meerkat_core::PeerIngressAuthorityPhase {
    fn from(phase: PeerIngressAuthorityPhaseClass) -> Self {
        match phase {
            PeerIngressAuthorityPhaseClass::Absent => Self::Absent,
            PeerIngressAuthorityPhaseClass::Received => Self::Received,
            PeerIngressAuthorityPhaseClass::Dropped => Self::Dropped,
            PeerIngressAuthorityPhaseClass::Delivered => Self::Delivered,
        }
    }
}

/// DSL-owned receive/admission result for classified peer ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressReceiveOutcomeClass {
    #[default]
    Admitted,
    DroppedUntrustedSender,
    DroppedSessionClosed,
    DroppedInboxFull,
}

impl From<PeerIngressReceiveOutcomeClass> for meerkat_core::PeerIngressReceiveOutcome {
    fn from(outcome: PeerIngressReceiveOutcomeClass) -> Self {
        match outcome {
            PeerIngressReceiveOutcomeClass::Admitted => Self::Admitted,
            PeerIngressReceiveOutcomeClass::DroppedUntrustedSender => Self::DroppedUntrustedSender,
            PeerIngressReceiveOutcomeClass::DroppedSessionClosed => Self::DroppedSessionClosed,
            PeerIngressReceiveOutcomeClass::DroppedInboxFull => Self::DroppedInboxFull,
        }
    }
}

/// DSL-owned admission diagnostic copy emitted with receive authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerIngressAdmissionDiagnosticClass {
    #[default]
    TrustedAtAdmission,
    UntrustedAtAdmission,
}

impl From<PeerIngressAdmissionDiagnosticClass> for meerkat_core::PeerIngressAdmissionDiagnostic {
    fn from(diagnostic: PeerIngressAdmissionDiagnosticClass) -> Self {
        match diagnostic {
            PeerIngressAdmissionDiagnosticClass::TrustedAtAdmission => Self::TrustedAtAdmission,
            PeerIngressAdmissionDiagnosticClass::UntrustedAtAdmission => Self::UntrustedAtAdmission,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredInputObservedPhase {
    Accepted,
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

/// Public lifecycle class emitted by generated authority before runtime
/// surfaces project input state onto their transport enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputPublicLifecycleState {
    #[default]
    Accepted,
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

impl InputPublicLifecycleState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "Accepted",
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

/// Public terminal result class emitted by generated authority before runtime
/// surfaces project input state onto their transport enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputPublicTerminalOutcome {
    #[default]
    Completed,
    Abandoned,
    Superseded,
    Coalesced,
    Cancelled,
}

impl InputPublicTerminalOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Abandoned => "Abandoned",
            Self::Superseded => "Superseded",
            Self::Coalesced => "Coalesced",
            Self::Cancelled => "Cancelled",
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

/// Typed turn primitive content shape. Closed mirror of
/// [`meerkat_core::turn_execution_authority::ContentShape`] so the runtime DSL
/// carries the same contract instead of local string labels.
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

impl ContentShape {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => {
                meerkat_core::turn_execution_authority::ContentShape::Conversation.as_str()
            }
            Self::ConversationAndContext => {
                meerkat_core::turn_execution_authority::ContentShape::ConversationAndContext
                    .as_str()
            }
            Self::Context => meerkat_core::turn_execution_authority::ContentShape::Context.as_str(),
            Self::Empty => meerkat_core::turn_execution_authority::ContentShape::Empty.as_str(),
            Self::ImmediateAppend => {
                meerkat_core::turn_execution_authority::ContentShape::ImmediateAppend.as_str()
            }
            Self::ImmediateContext => {
                meerkat_core::turn_execution_authority::ContentShape::ImmediateContext.as_str()
            }
        }
    }
}

impl std::fmt::Display for ContentShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<meerkat_core::turn_execution_authority::ContentShape> for ContentShape {
    fn from(shape: meerkat_core::turn_execution_authority::ContentShape) -> Self {
        match shape {
            meerkat_core::turn_execution_authority::ContentShape::Conversation => {
                Self::Conversation
            }
            meerkat_core::turn_execution_authority::ContentShape::ConversationAndContext => {
                Self::ConversationAndContext
            }
            meerkat_core::turn_execution_authority::ContentShape::Context => Self::Context,
            meerkat_core::turn_execution_authority::ContentShape::Empty => Self::Empty,
            meerkat_core::turn_execution_authority::ContentShape::ImmediateAppend => {
                Self::ImmediateAppend
            }
            meerkat_core::turn_execution_authority::ContentShape::ImmediateContext => {
                Self::ImmediateContext
            }
        }
    }
}

impl From<ContentShape> for meerkat_core::turn_execution_authority::ContentShape {
    fn from(shape: ContentShape) -> Self {
        match shape {
            ContentShape::Conversation => Self::Conversation,
            ContentShape::ConversationAndContext => Self::ConversationAndContext,
            ContentShape::Context => Self::Context,
            ContentShape::Empty => Self::Empty,
            ContentShape::ImmediateAppend => Self::ImmediateAppend,
            ContentShape::ImmediateContext => Self::ImmediateContext,
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

impl From<meerkat_core::turn_execution_authority::TurnTerminalCauseKind> for TurnTerminalCauseKind {
    fn from(kind: meerkat_core::turn_execution_authority::TurnTerminalCauseKind) -> Self {
        match kind {
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::Unknown => {
                Self::Unknown
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::HookDenied => {
                Self::HookDenied
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::HookFailure => {
                Self::HookFailure
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::LlmFailure => {
                Self::LlmFailure
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::ToolFailure => {
                Self::ToolFailure
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::StructuredOutputValidationFailed => {
                Self::StructuredOutputValidationFailed
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::BudgetExhausted => {
                Self::BudgetExhausted
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::TimeBudgetExceeded => {
                Self::TimeBudgetExceeded
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::RetryExhausted => {
                Self::RetryExhausted
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::TurnLimitReached => {
                Self::TurnLimitReached
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::RuntimeApplyFailure => {
                Self::RuntimeApplyFailure
            }
            meerkat_core::turn_execution_authority::TurnTerminalCauseKind::FatalFailure => {
                Self::FatalFailure
            }
        }
    }
}

impl From<TurnTerminalCauseKind> for meerkat_core::turn_execution_authority::TurnTerminalCauseKind {
    fn from(kind: TurnTerminalCauseKind) -> Self {
        match kind {
            TurnTerminalCauseKind::Unknown => Self::Unknown,
            TurnTerminalCauseKind::HookDenied => Self::HookDenied,
            TurnTerminalCauseKind::HookFailure => Self::HookFailure,
            TurnTerminalCauseKind::LlmFailure => Self::LlmFailure,
            TurnTerminalCauseKind::ToolFailure => Self::ToolFailure,
            TurnTerminalCauseKind::StructuredOutputValidationFailed => {
                Self::StructuredOutputValidationFailed
            }
            TurnTerminalCauseKind::BudgetExhausted => Self::BudgetExhausted,
            TurnTerminalCauseKind::TimeBudgetExceeded => Self::TimeBudgetExceeded,
            TurnTerminalCauseKind::RetryExhausted => Self::RetryExhausted,
            TurnTerminalCauseKind::TurnLimitReached => Self::TurnLimitReached,
            TurnTerminalCauseKind::RuntimeApplyFailure => Self::RuntimeApplyFailure,
            TurnTerminalCauseKind::FatalFailure => Self::FatalFailure,
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
    HookDenied,
    HookRuntimeFailure,
    ExecutorStopped,
    ExecutorControlFailed,
    ExecutorInternal,
}

impl From<meerkat_core::lifecycle::CoreApplyFailureCauseKind> for RuntimeApplyFailureCause {
    #[allow(clippy::panic)]
    fn from(kind: meerkat_core::lifecycle::CoreApplyFailureCauseKind) -> Self {
        match kind {
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::PrimitiveRejected => {
                Self::PrimitiveRejected
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::RuntimeContextApply => {
                Self::RuntimeContextApply
            }
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::RuntimeTurn => Self::RuntimeTurn,
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::HookDenied => Self::HookDenied,
            meerkat_core::lifecycle::CoreApplyFailureCauseKind::HookRuntimeFailure => {
                Self::HookRuntimeFailure
            }
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
            _ => panic!(
                "unsupported CoreApplyFailureCauseKind variant; update generated MeerkatMachine mirror"
            ),
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

/// Closed classifier for runtime-loop executor effects emitted as neutral DSL
/// facts before the runtime shell converts them to sealed executable effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeEffectKind {
    #[default]
    CancelAfterBoundary,
    StopRuntimeExecutor,
}

/// Typed public result class for `live/refresh` after the adapter command
/// queue accepts a refresh handoff. The RPC surface may only project this
/// value from a generated `LiveRefreshResultResolved` effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveRefreshPublicStatus {
    #[default]
    Queued,
}

/// Typed public result class for `live/close` after the live host accepts a
/// close handoff. The RPC surface may only project this value from a generated
/// `LiveCloseResultResolved` effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveClosePublicStatus {
    #[default]
    Closed,
}

/// Typed terminal reason for RPC event streams. The router observes transport
/// end conditions, then submits the closed set here before projecting the
/// public `*/stream_end` notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RpcEventStreamTerminalReason {
    #[default]
    RemoteEnd,
    TerminalError,
    ExplicitClose,
}

/// Typed public status class for `live/status` after the live host has
/// observed the adapter transport state. RPC/SDK surfaces may only project
/// these values from generated `LiveChannelStatusResolved` effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveChannelPublicStatus {
    #[default]
    Idle,
    Opening,
    Ready,
    Degraded,
    Closing,
    Closed,
}

/// Typed public degradation reason for `live/status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LiveChannelDegradationReason {
    #[default]
    Unknown,
    RateLimited,
    ProviderThrottled,
    NetworkUnstable,
    Other,
}

/// Typed mirror of the public runtime lifecycle projection. The shell passes
/// only the observed variant; generated transitions own the semantic facts
/// derived from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeLifecycleObservedState {
    #[default]
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeLifecycleTerminality {
    #[default]
    NonTerminal,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeInputAdmission {
    #[default]
    RejectsInput,
    AcceptsInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeQueueAdmission {
    #[default]
    BlocksQueue,
    ProcessesQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimePrepareAdmission {
    #[default]
    NotReady,
    Ready,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeIngressAdmission {
    #[default]
    Open,
    NotReady,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeLoopRunBinding {
    #[default]
    Blocked,
    AllocateNew,
    UsePrebound,
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

/// Generated surface-request lifecycle phase. Surface transports may project
/// this value for diagnostics; mutation authority lives in MeerkatMachine
/// transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SurfaceRequestPhase {
    #[default]
    Pending,
    Published,
    Cancelled,
    Completed,
}

/// Generated terminal-publication policy recorded when a surface request is
/// admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SurfaceRequestTerminalPolicy {
    #[default]
    RespondWithoutPublish,
    PublishOnSuccess,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OpRegistrationAdmissionResultKind {
    #[default]
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OpRegistrationRejectReasonKind {
    #[default]
    AlreadyRegistered,
    MaxConcurrentExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OpLifecycleActionKind {
    #[default]
    Start,
    Fail,
    PeerReady,
    ProgressReported,
    Complete,
    Abort,
    Cancel,
    RetireRequested,
    RetireCompleted,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OpLifecycleRejectReasonKind {
    #[default]
    OperationNotFound,
    InvalidTransition,
    PeerNotExpected,
    AlreadyPeerReady,
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

/// Typed live-admission input kind carried by `ResolveAdmissionPlan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionInputKind {
    #[default]
    Prompt,
    PeerMessage,
    PeerRequest,
    PeerResponseProgress,
    PeerResponseTerminal,
    FlowStep,
    ExternalEvent,
    Continuation,
    Operation,
}

/// Typed durability class observed on an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum InputDurabilityKind {
    #[default]
    Durable,
    Ephemeral,
    Derived,
    Missing,
}

impl From<crate::input::InputDurability> for InputDurabilityKind {
    fn from(durability: crate::input::InputDurability) -> Self {
        match durability {
            crate::input::InputDurability::Durable => Self::Durable,
            crate::input::InputDurability::Ephemeral => Self::Ephemeral,
            crate::input::InputDurability::Derived => Self::Derived,
        }
    }
}

impl From<Option<crate::input::InputDurability>> for InputDurabilityKind {
    fn from(durability: Option<crate::input::InputDurability>) -> Self {
        durability.map(Self::from).unwrap_or(Self::Missing)
    }
}

/// Typed input-origin class observed at live admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionInputOriginKind {
    #[default]
    Operator,
    Peer,
    Flow,
    System,
    External,
}

impl From<&crate::input::InputOrigin> for AdmissionInputOriginKind {
    fn from(origin: &crate::input::InputOrigin) -> Self {
        match origin {
            crate::input::InputOrigin::Operator => Self::Operator,
            crate::input::InputOrigin::Peer { .. } => Self::Peer,
            crate::input::InputOrigin::Flow { .. } => Self::Flow,
            crate::input::InputOrigin::System => Self::System,
            crate::input::InputOrigin::External { .. } => Self::External,
        }
    }
}

impl From<crate::identifiers::InputKind> for AdmissionInputKind {
    fn from(kind: crate::identifiers::InputKind) -> Self {
        match kind {
            crate::identifiers::InputKind::Prompt => Self::Prompt,
            crate::identifiers::InputKind::PeerMessage => Self::PeerMessage,
            crate::identifiers::InputKind::PeerRequest => Self::PeerRequest,
            crate::identifiers::InputKind::PeerResponseProgress => Self::PeerResponseProgress,
            crate::identifiers::InputKind::PeerResponseTerminal => Self::PeerResponseTerminal,
            crate::identifiers::InputKind::FlowStep => Self::FlowStep,
            crate::identifiers::InputKind::ExternalEvent => Self::ExternalEvent,
            crate::identifiers::InputKind::Continuation => Self::Continuation,
            crate::identifiers::InputKind::Operation => Self::Operation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPolicyApplyMode {
    #[default]
    StageRunStart,
    StageRunBoundary,
    InjectNow,
    Ignore,
}

impl From<AdmissionPolicyApplyMode> for crate::policy::ApplyMode {
    fn from(mode: AdmissionPolicyApplyMode) -> Self {
        match mode {
            AdmissionPolicyApplyMode::StageRunStart => Self::StageRunStart,
            AdmissionPolicyApplyMode::StageRunBoundary => Self::StageRunBoundary,
            AdmissionPolicyApplyMode::InjectNow => Self::InjectNow,
            AdmissionPolicyApplyMode::Ignore => Self::Ignore,
        }
    }
}

impl From<crate::policy::ApplyMode> for AdmissionPolicyApplyMode {
    fn from(mode: crate::policy::ApplyMode) -> Self {
        match mode {
            crate::policy::ApplyMode::StageRunStart => Self::StageRunStart,
            crate::policy::ApplyMode::StageRunBoundary => Self::StageRunBoundary,
            crate::policy::ApplyMode::InjectNow => Self::InjectNow,
            crate::policy::ApplyMode::Ignore => Self::Ignore,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPolicyWakeMode {
    #[default]
    WakeIfIdle,
    InterruptYielding,
    None,
}

impl From<AdmissionPolicyWakeMode> for crate::policy::WakeMode {
    fn from(mode: AdmissionPolicyWakeMode) -> Self {
        match mode {
            AdmissionPolicyWakeMode::WakeIfIdle => Self::WakeIfIdle,
            AdmissionPolicyWakeMode::InterruptYielding => Self::InterruptYielding,
            AdmissionPolicyWakeMode::None => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPolicyQueueMode {
    None,
    #[default]
    Fifo,
    Coalesce,
    Supersede,
    Priority,
}

impl From<AdmissionPolicyQueueMode> for crate::policy::QueueMode {
    fn from(mode: AdmissionPolicyQueueMode) -> Self {
        match mode {
            AdmissionPolicyQueueMode::None => Self::None,
            AdmissionPolicyQueueMode::Fifo => Self::Fifo,
            AdmissionPolicyQueueMode::Coalesce => Self::Coalesce,
            AdmissionPolicyQueueMode::Supersede => Self::Supersede,
            AdmissionPolicyQueueMode::Priority => Self::Priority,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPolicyConsumePoint {
    OnAccept,
    OnApply,
    OnRunStart,
    #[default]
    OnRunComplete,
    ExplicitAck,
}

impl From<AdmissionPolicyConsumePoint> for crate::policy::ConsumePoint {
    fn from(point: AdmissionPolicyConsumePoint) -> Self {
        match point {
            AdmissionPolicyConsumePoint::OnAccept => Self::OnAccept,
            AdmissionPolicyConsumePoint::OnApply => Self::OnApply,
            AdmissionPolicyConsumePoint::OnRunStart => Self::OnRunStart,
            AdmissionPolicyConsumePoint::OnRunComplete => Self::OnRunComplete,
            AdmissionPolicyConsumePoint::ExplicitAck => Self::ExplicitAck,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPolicyDrainPolicy {
    #[default]
    QueueNextTurn,
    SteerBatch,
    Immediate,
    Ignore,
}

impl From<AdmissionPolicyDrainPolicy> for crate::policy::DrainPolicy {
    fn from(policy: AdmissionPolicyDrainPolicy) -> Self {
        match policy {
            AdmissionPolicyDrainPolicy::QueueNextTurn => Self::QueueNextTurn,
            AdmissionPolicyDrainPolicy::SteerBatch => Self::SteerBatch,
            AdmissionPolicyDrainPolicy::Immediate => Self::Immediate,
            AdmissionPolicyDrainPolicy::Ignore => Self::Ignore,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionRoutingDisposition {
    #[default]
    Queue,
    Steer,
    Immediate,
    Drop,
}

impl From<AdmissionRoutingDisposition> for crate::policy::RoutingDisposition {
    fn from(disposition: AdmissionRoutingDisposition) -> Self {
        match disposition {
            AdmissionRoutingDisposition::Queue => Self::Queue,
            AdmissionRoutingDisposition::Steer => Self::Steer,
            AdmissionRoutingDisposition::Immediate => Self::Immediate,
            AdmissionRoutingDisposition::Drop => Self::Drop,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionRunApplyBoundary {
    #[default]
    RunStart,
    RunCheckpoint,
    Immediate,
}

impl From<AdmissionRunApplyBoundary> for meerkat_core::lifecycle::run_primitive::RunApplyBoundary {
    fn from(boundary: AdmissionRunApplyBoundary) -> Self {
        match boundary {
            AdmissionRunApplyBoundary::RunStart => Self::RunStart,
            AdmissionRunApplyBoundary::RunCheckpoint => Self::RunCheckpoint,
            AdmissionRunApplyBoundary::Immediate => Self::Immediate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionRuntimeExecutionKind {
    #[default]
    ContentTurn,
    ResumePending,
}

impl From<AdmissionRuntimeExecutionKind> for meerkat_core::lifecycle::RuntimeExecutionKind {
    fn from(kind: AdmissionRuntimeExecutionKind) -> Self {
        match kind {
            AdmissionRuntimeExecutionKind::ContentTurn => Self::ContentTurn,
            AdmissionRuntimeExecutionKind::ResumePending => Self::ResumePending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPeerResponseTerminalApplyIntent {
    #[default]
    AppendContextAndRun,
}

impl From<AdmissionPeerResponseTerminalApplyIntent>
    for meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent
{
    fn from(intent: AdmissionPeerResponseTerminalApplyIntent) -> Self {
        match intent {
            AdmissionPeerResponseTerminalApplyIntent::AppendContextAndRun => {
                Self::AppendContextAndRun
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionPlanKind {
    ConsumedOnAccept,
    #[default]
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionIdempotencyResultKind {
    #[default]
    Accept,
    Deduplicated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionValidationResultKind {
    #[default]
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PeerResponseTerminalObservedStatus {
    #[default]
    NotPeerTerminal,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionRejectReasonKind {
    #[default]
    DurabilityViolation,
    PeerHandlingModeInvalid,
    PeerResponseTerminalInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WaitAllAdmissionResultKind {
    #[default]
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WaitAllRejectReasonKind {
    #[default]
    DuplicateOperation,
    WaitAlreadyActive,
    OperationNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredInputNormalizationReasonKind {
    #[default]
    QueueAccepted,
    RollbackStaged,
    BoundaryReceiptCommitted,
    MissingBoundaryReceipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionQueueActionKind {
    #[default]
    None,
    EnqueueTo,
    EnqueueFront,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdmissionExistingQueuedActionKind {
    #[default]
    None,
    Coalesce,
    Supersede,
}

/// Typed persisted input kind carried by recovered-admission witnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredInputKind {
    #[default]
    Prompt,
    PeerMessage,
    PeerRequest,
    PeerResponseProgress,
    PeerResponseTerminal,
    FlowStep,
    ExternalEvent,
    Continuation,
    Operation,
}

/// Generated recovery disposition for a persisted input row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredInputRecoveryDisposition {
    #[default]
    Retain,
    Discard,
}

impl From<crate::identifiers::InputKind> for RecoveredInputKind {
    fn from(kind: crate::identifiers::InputKind) -> Self {
        match kind {
            crate::identifiers::InputKind::Prompt => Self::Prompt,
            crate::identifiers::InputKind::PeerMessage => Self::PeerMessage,
            crate::identifiers::InputKind::PeerRequest => Self::PeerRequest,
            crate::identifiers::InputKind::PeerResponseProgress => Self::PeerResponseProgress,
            crate::identifiers::InputKind::PeerResponseTerminal => Self::PeerResponseTerminal,
            crate::identifiers::InputKind::FlowStep => Self::FlowStep,
            crate::identifiers::InputKind::ExternalEvent => Self::ExternalEvent,
            crate::identifiers::InputKind::Continuation => Self::Continuation,
            crate::identifiers::InputKind::Operation => Self::Operation,
        }
    }
}

/// Typed persisted runtime apply boundary carried by recovered-admission
/// witnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredRunApplyBoundary {
    #[default]
    RunStart,
    RunCheckpoint,
    Immediate,
}

impl TryFrom<meerkat_core::lifecycle::run_primitive::RunApplyBoundary>
    for RecoveredRunApplyBoundary
{
    type Error = &'static str;

    fn try_from(
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
    ) -> Result<Self, Self::Error> {
        match boundary {
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart => {
                Ok(Self::RunStart)
            }
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint => {
                Ok(Self::RunCheckpoint)
            }
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate => {
                Ok(Self::Immediate)
            }
            _ => Err("unknown recovered runtime boundary"),
        }
    }
}

/// Typed persisted runtime execution class carried by recovered-admission
/// witnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredRuntimeExecutionKind {
    #[default]
    ContentTurn,
    ResumePending,
}

impl From<meerkat_core::lifecycle::RuntimeExecutionKind> for RecoveredRuntimeExecutionKind {
    fn from(kind: meerkat_core::lifecycle::RuntimeExecutionKind) -> Self {
        match kind {
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn => Self::ContentTurn,
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending => Self::ResumePending,
        }
    }
}

/// Typed recovered terminal peer-response apply intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RecoveredPeerResponseTerminalApplyIntent {
    #[default]
    AppendContextAndRun,
}

impl From<meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent>
    for RecoveredPeerResponseTerminalApplyIntent
{
    fn from(
        intent: meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent,
    ) -> Self {
        match intent {
            meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent::AppendContextAndRun => {
                Self::AppendContextAndRun
            }
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
pub enum RoutingSwitchApprovalReason {
    #[default]
    CrossProvider,
    CostExceedsThreshold,
    SafetyHold,
    UntilChangedFromModelOrigin,
    RealtimeDetachRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingImageApprovalReason {
    #[default]
    CrossProvider,
    CostExceedsThreshold,
    SafetyHold,
    RealtimeDetachRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoutingImagePlanDenialReason {
    #[default]
    UnsupportedTarget,
    UnsupportedCount,
    CapabilityPolicy,
    CostPolicy,
    SafetyPolicy,
    ApprovalRequiredButUnavailable,
    DeniedDuringApproval,
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
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
