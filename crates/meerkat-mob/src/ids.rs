//! Newtype identifiers for mob entities.
//!
//! These types wrap concrete primitives for compile-time safety.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Unique identifier for a flow run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub(crate) fn for_external_delivery(
        mob_id: &MobId,
        flow_id: &FlowId,
        idempotency_key: &str,
    ) -> Self {
        let mut name = b"meerkat.mob.external-delivery.v1".to_vec();
        for component in [mob_id.as_str(), flow_id.as_str(), idempotency_key] {
            let bytes = component.as_bytes();
            name.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            name.extend_from_slice(bytes);
        }
        Self(Uuid::new_v5(&Uuid::NAMESPACE_URL, &name))
    }

    /// Deterministic Flow run identity for a caller-stable external delivery.
    ///
    /// Public composition layers use this to persist the exact run identity
    /// before invoking Mob. `run_flow_with_external_identity` derives the same
    /// value and verifies that a replay cannot select another run.
    pub fn for_work_execution(mob_id: &MobId, flow_id: &FlowId, idempotency_key: &str) -> Self {
        Self::for_external_delivery(mob_id, flow_id, idempotency_key)
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RunId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Controller-minted identity for one placed-member spawn attempt.
///
/// Unlike member generation, this id is never reused after an aborted attempt.
/// It is the ABA fence shared by the generated MobMachine handoff, the durable
/// placed-spawn carrier, and the internal stored-event projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlacedSpawnId(Uuid);

impl PlacedSpawnId {
    /// Mint a fresh controller-owned placed-spawn attempt id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for PlacedSpawnId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PlacedSpawnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for PlacedSpawnId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(value)?))
    }
}

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<String> for $name {
            fn borrow(&self) -> &String {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }

        impl PartialEq<&String> for $name {
            fn eq(&self, other: &&String) -> bool {
                &self.0 == *other
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0.as_str() == *other
            }
        }
    };
}

string_newtype!(
    /// Unique identifier for a mob instance.
    MobId
);

string_newtype!(
    /// Unique identifier for a flow definition.
    FlowId
);

string_newtype!(
    /// Unique identifier for a step in a flow definition.
    StepId
);

string_newtype!(
    /// Branch group identifier used by mutually-exclusive flow steps.
    BranchId
);

string_newtype!(
    /// Profile name within a mob definition.
    ProfileName
);

string_newtype!(
    /// Runtime identifier for a flow execution frame. One per FrameSpec invocation.
    FrameId
);

string_newtype!(
    /// Runtime identifier for one instance of a repeat_until loop.
    LoopInstanceId
);

string_newtype!(
    /// Lexical identifier for a node within a FrameSpec.
    FlowNodeId
);

string_newtype!(
    /// Lexical identifier for a loop definition within a FrameSpec.
    LoopId
);

// ---------------------------------------------------------------------------
// Identity-first mob model types (0.6)
// ---------------------------------------------------------------------------

string_newtype!(
    /// Stable, human-meaningful identity for a mob member.
    ///
    /// An `AgentIdentity` is assigned at spawn and persists across respawns and
    /// resets. It is the canonical key for all public mob APIs.
    AgentIdentity
);

string_newtype!(
    /// Canonical peer identity carried in respawn topology-restore feedback.
    ///
    /// Local member edges use the member's [`AgentIdentity`] string. External
    /// peer edges use the comms [`meerkat_core::comms::PeerId`] string, never
    /// the display-only peer name.
    RespawnTopologyPeerId
);

impl AgentIdentity {
    /// Stable synthetic lead principal for an owner bridge session that is
    /// not itself a roster member (implicit delegation mobs).
    pub fn objective_lead_for_session(session_id: &meerkat_core::SessionId) -> Self {
        Self::from(format!("objective-lead-session:{session_id}"))
    }

    /// Returns `true` when this identity falls inside the reserved
    /// flow-owned member namespace.
    ///
    /// This is the single canonical predicate for "is this a system-owned
    /// identity the public spawn/wire surfaces must reject"; all admission and
    /// validation sites route through it rather than re-deriving the meaning
    /// from the underlying string prefix.
    pub(crate) fn is_system_reserved(&self) -> bool {
        self.as_str()
            .starts_with(crate::runtime::FLOW_MEMBER_ID_PREFIX)
    }

    /// Returns `true` when this identity falls inside the flow-owned member
    /// namespace. Only MobMachine-owned flow provisioning may mint these
    /// identities; public spawn surfaces must reject them.
    pub(crate) fn is_flow_member_namespace(&self) -> bool {
        self.is_system_reserved()
    }

    /// Constructs the reserved provenance identity used to attribute
    /// flow-system-authored step ledger entries.
    pub(crate) fn flow_system_provenance() -> Self {
        Self::from(crate::run::FLOW_RUN_PROVENANCE_AGENT_ID)
    }

    /// Returns `true` when this identity is exactly the reserved flow-system
    /// provenance identity (stricter than [`Self::is_system_reserved`], which
    /// matches the whole reserved namespace).
    pub(crate) fn is_flow_system_provenance(&self) -> bool {
        self.as_str() == crate::run::FLOW_RUN_PROVENANCE_AGENT_ID
    }
}

// ---------------------------------------------------------------------------
// The one encoding of a name into a member id
// ---------------------------------------------------------------------------

/// The marker that begins a member id encoding a name
/// ([`member_id_for_name`]): a reserved namespace, so a name that begins
/// with it is encoded too and every name round-trips.
pub const ENCODED_MEMBER_ID_MARKER: &str = "mk--";

/// Whether `name` is a member comms-name component: an ASCII letter or `_`,
/// then ASCII alphanumerics, `-` or `_` (the rule each component of a
/// member's comms name keeps, [`meerkat_core::connection::MemberCommsName`]).
fn is_member_comms_component(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `name` escaped: `_` as `__`, `:` as `_c`, and any other character
/// outside `[A-Za-z0-9-]` as `_x{hex}_`.
fn escape_member_name(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len() + 8);
    for c in name.chars() {
        match c {
            '_' => escaped.push_str("__"),
            ':' => escaped.push_str("_c"),
            c if c.is_ascii_alphanumeric() || c == '-' => escaped.push(c),
            c => escaped.push_str(&format!("_x{:x}_", u32::from(c))),
        }
    }
    escaped
}

/// The name an escaped body spells ([`escape_member_name`]); `None` when the
/// body is no escape production, which no encoding produced.
fn unescape_member_name(body: &str) -> Option<String> {
    let mut name = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '_' {
            name.push(c);
            continue;
        }
        match chars.next()? {
            '_' => name.push('_'),
            'c' => name.push(':'),
            'x' => {
                let mut hex = String::new();
                loop {
                    match chars.next()? {
                        '_' => break,
                        digit => hex.push(digit),
                    }
                }
                let code = u32::from_str_radix(&hex, 16).ok()?;
                name.push(char::from_u32(code)?);
            }
            _ => return None,
        }
    }
    Some(name)
}

/// The member id for `name`, in the one encoding of a name into a member id
/// ([`AgentIdentity`]): `name` itself when it is a member comms-name
/// component (an ASCII letter or `_`, then ASCII alphanumerics, `-` or `_`)
/// that does not begin with [`ENCODED_MEMBER_ID_MARKER`]; otherwise the
/// marker and the escaped name, where `_` is `__`, `:` is `_c` and any
/// other character outside `[A-Za-z0-9-]` is `_x{hex}_`.
/// [`name_for_member_id`] inverts it.
///
/// A host that names its identities in a form no member id can spell
/// (`team:alice`) seats each under this id, and the legacy adoption reads a
/// member's identity label through it.
pub fn member_id_for_name(name: &str) -> std::borrow::Cow<'_, str> {
    if is_member_comms_component(name) && !name.starts_with(ENCODED_MEMBER_ID_MARKER) {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(format!(
            "{ENCODED_MEMBER_ID_MARKER}{}",
            escape_member_name(name)
        ))
    }
}

/// The name `member_id` encodes ([`member_id_for_name`]): the member id
/// itself when it is no encoding (it does not begin with
/// [`ENCODED_MEMBER_ID_MARKER`], or what follows the marker is no escape
/// production).
pub fn name_for_member_id(member_id: &str) -> std::borrow::Cow<'_, str> {
    match decode_member_id(member_id) {
        Some(name) => std::borrow::Cow::Owned(name),
        None => std::borrow::Cow::Borrowed(member_id),
    }
}

/// The name an encoded member id spells ([`member_id_for_name`]): `None`
/// when `member_id` does not begin with [`ENCODED_MEMBER_ID_MARKER`], or
/// what follows the marker is no escape production, which no encoding
/// produced. The one decoder of the encoding.
pub(crate) fn decode_member_id(member_id: &str) -> Option<String> {
    member_id
        .strip_prefix(ENCODED_MEMBER_ID_MARKER)
        .and_then(unescape_member_name)
}

/// Monotonically increasing generation counter for a mob member.
///
/// Starts at 0 on first spawn, advances on each reset. The generation is
/// part of [`AgentRuntimeId`] and disambiguates successive incarnations of
/// the same [`AgentIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(u64);

impl Generation {
    /// The initial generation assigned to a freshly spawned member.
    pub const INITIAL: Self = Self(0);

    /// Create a generation from a raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the underlying value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Advance to the next generation, or return `None` when the identity has
    /// exhausted the `u64` generation space. Generation zero is never reused.
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Unique runtime identity for a specific incarnation of a mob member.
///
/// Combines the stable [`AgentIdentity`] with a [`Generation`] counter that
/// advances on reset. Two `AgentRuntimeId` values with the same identity but
/// different generations represent successive incarnations of the same member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentRuntimeId {
    /// Stable member identity.
    pub identity: AgentIdentity,
    /// Generation counter for this incarnation.
    pub generation: Generation,
}

impl AgentRuntimeId {
    /// Create a new runtime id.
    pub fn new(identity: AgentIdentity, generation: Generation) -> Self {
        Self {
            identity,
            generation,
        }
    }

    /// Create an initial runtime id (generation 0).
    pub fn initial(identity: AgentIdentity) -> Self {
        Self {
            identity,
            generation: Generation::INITIAL,
        }
    }
}

impl fmt::Display for AgentRuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.identity, self.generation.get())
    }
}

/// Opaque fence token used to reject stale commands.
///
/// A new `FenceToken` is issued at spawn, respawn, and reset. Commands
/// carrying a stale token are rejected, preventing races where a delayed
/// message targets an incarnation that has already been replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FenceToken(u64);

impl FenceToken {
    /// Create a fence token from a raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the underlying value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FenceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fence:{}", self.0)
    }
}

/// Unique identifier for a unit of work submitted to a mob member.
///
/// Analogous to [`RunId`] but scoped to the work-lane abstraction introduced
/// in the identity-first model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkRef(Uuid);

impl WorkRef {
    /// Generate a new random work reference.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub(crate) fn for_delivery(
        mob_id: &MobId,
        member_id: &AgentIdentity,
        idempotency_key: &str,
    ) -> Self {
        Self(Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("meerkat:mob:{mob_id}:member:{member_id}:delivery:{idempotency_key}")
                .as_bytes(),
        ))
    }

    pub(crate) fn for_external_delivery(
        mob_id: &MobId,
        member_id: &AgentIdentity,
        idempotency_key: &str,
    ) -> Self {
        Self::for_delivery(mob_id, member_id, idempotency_key)
    }
}

impl Default for WorkRef {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for WorkRef {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Describes a unit of work to be executed by a mob member.
///
/// `WorkSpec` is submitted alongside a [`WorkRef`] and [`FenceToken`] through
/// the work lane. It captures the content and delivery semantics without
/// exposing session-level details.
///
/// DELETE_ME C6: `content` is a full [`meerkat_core::types::ContentInput`]
/// (multimodal) rather than `String`, matching the rest of the platform's
/// content-carrying types. Prior to this change the work lane was silently
/// text-only, which was a capability regression vs. every other member-
/// delivery surface. `impl From<String> for ContentInput` / `From<&str>` in
/// `meerkat_core` means existing String call sites upgrade without
/// per-call-site conversion noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkSpec {
    /// The content to deliver to the member.
    pub content: meerkat_core::types::ContentInput,
    /// Whether this is an externally-originated turn (user input) or an
    /// internally-originated turn (mob coordination).
    pub origin: WorkOrigin,
    /// Optional ordinary System message authored for this exact member turn.
    ///
    /// This is per-turn content, not immutable member/session configuration:
    /// repeated submissions may carry different values, and each present
    /// value is appended at that turn's admitted transcript boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Host-attached injected context delivered alongside (not inside) the
    /// work content. Each entry materializes as a separate typed
    /// injected-context transcript message immediately before the work
    /// content, in order. Deliverable on queue-mode turn-driven dispatch
    /// (local and remote); autonomous inbox delivery and steer-mode dispatch
    /// reject it fail-closed with a typed [`crate::MobError`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context: Vec<meerkat_core::types::ContentInput>,
    /// Host-regenerated request-only context for the member's logical turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transient_turn_context: Option<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
    /// Host-supplied interaction identity for this unit of work.
    ///
    /// Ask-15 addendum: the delivery path carries this id to
    /// `RuntimeTurnMetadata.transcript_identity` (turn-driven dispatch) or
    /// stamps it onto the injected inbox event (autonomous dispatch), so the
    /// transcript messages committed for this turn persist the SAME
    /// interaction id the host's live `interaction_complete` frames carry —
    /// history backfill can then join persisted and live frames without
    /// content heuristics. Serde-additive: absent on old stored shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<meerkat_core::interaction::InteractionId>,
    /// Durable objective correlation propagated through the work lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    /// The reasoning-effort preference a live delegation's sealed admission
    /// carries for this member turn (#1823). Set only inside this crate from
    /// that admission; never accepted from a caller or a wire. The admitted
    /// runtime input persists it.
    #[serde(skip)]
    pub(crate) request_reasoning:
        Option<meerkat_core::lifecycle::run_primitive::RequestReasoningPreference>,
    /// The original requester of this work: an opaque, qualified principal
    /// the host authenticated, never a request body, a tool argument or
    /// model output. Only host human input carries one (the member's runtime
    /// stores it with the admitted input and its run hands it to tool
    /// dispatch); other work that carries one is refused. Set only through
    /// [`Self::with_requester`]; never accepted from a wire.
    #[serde(skip)]
    pub(crate) requester: Option<meerkat_core::PrincipalRef>,
}

impl WorkSpec {
    /// Create a new work spec. Accepts anything that implements
    /// `Into<ContentInput>` — including `String` and `&str` — so existing
    /// text-only call sites upgrade without churn.
    pub fn new(content: impl Into<meerkat_core::types::ContentInput>, origin: WorkOrigin) -> Self {
        Self {
            content: content.into(),
            origin,
            system_prompt: None,
            injected_context: Vec::new(),
            transient_turn_context: None,
            interaction_id: None,
            objective_id: None,
            request_reasoning: None,
            requester: None,
        }
    }

    /// Carry the original requester the host authenticated for this work
    /// (see the `requester` field): only host human input may carry one.
    #[must_use]
    pub fn with_requester(mut self, requester: meerkat_core::PrincipalRef) -> Self {
        self.requester = Some(requester);
        self
    }

    /// The original requester this work carries, if any.
    pub fn requester(&self) -> Option<&meerkat_core::PrincipalRef> {
        self.requester.as_ref()
    }

    /// Carry a live delegation admission's reasoning preference.
    #[must_use]
    pub(crate) fn with_request_reasoning(
        mut self,
        preference: Option<meerkat_core::lifecycle::run_primitive::RequestReasoningPreference>,
    ) -> Self {
        self.request_reasoning = preference;
        self
    }

    /// Attach one ordinary System message to this exact member turn.
    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    #[must_use]
    pub fn with_objective_id(
        mut self,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> Self {
        self.objective_id = Some(objective_id);
        self
    }

    /// Attach host-attached injected context to this work spec.
    pub fn with_injected_context(
        mut self,
        injected_context: Vec<meerkat_core::types::ContentInput>,
    ) -> Self {
        self.injected_context = injected_context;
        self
    }

    #[must_use]
    pub fn with_transient_turn_context(
        mut self,
        context: meerkat_core::lifecycle::run_primitive::TurnRequestContext,
    ) -> Self {
        self.transient_turn_context = Some(context);
        self
    }

    /// Attach a host-supplied interaction id so the committed transcript
    /// messages for this turn persist the same identity the host's live
    /// interaction frames carry.
    pub fn with_interaction_id(
        mut self,
        interaction_id: meerkat_core::interaction::InteractionId,
    ) -> Self {
        self.interaction_id = Some(interaction_id);
        self
    }
}

/// Origin classification for a [`WorkSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkOrigin {
    /// Externally-originated work (user or API surface).
    External,
    /// Internally-originated work (mob orchestration, flow engine).
    Internal,
}

impl WorkOrigin {
    /// Stable string label consumed by `MobMachine` DSL guards.
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkOrigin::External => "External",
            WorkOrigin::Internal => "Internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_spec_interaction_id_is_serde_additive() {
        // Absent field (the pre-ask-15 stored shape) deserializes to None,
        // and None is skipped on serialization so old readers see the old
        // shape unchanged.
        let json = serde_json::to_value(WorkSpec::new(
            "do the thing".to_string(),
            WorkOrigin::Internal,
        ))
        .unwrap();
        assert!(
            json.get("interaction_id").is_none(),
            "None must not serialize (serde-additive)"
        );
        let spec: WorkSpec = serde_json::from_value(json).unwrap();
        assert!(spec.interaction_id.is_none());

        // A carried id round-trips.
        let id = meerkat_core::interaction::InteractionId(Uuid::new_v4());
        let keyed =
            WorkSpec::new("do the thing".to_string(), WorkOrigin::External).with_interaction_id(id);
        let json = serde_json::to_value(&keyed).unwrap();
        let parsed: WorkSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.interaction_id, Some(id));
    }

    #[test]
    fn work_spec_system_prompt_is_per_turn_and_serde_additive() {
        let absent = WorkSpec::new("first turn", WorkOrigin::Internal);
        let absent_json = serde_json::to_value(&absent).unwrap();
        assert!(absent_json.get("system_prompt").is_none());

        let present = WorkSpec::new("second turn", WorkOrigin::Internal)
            .with_system_prompt("updated instructions");
        let json = serde_json::to_value(&present).unwrap();
        assert_eq!(json["system_prompt"], "updated instructions");
        let decoded: WorkSpec = serde_json::from_value(json).unwrap();
        assert_eq!(
            decoded.system_prompt.as_deref(),
            Some("updated instructions")
        );
    }

    #[test]
    fn test_run_id_roundtrip_json() {
        let run_id = RunId::new();
        let encoded = serde_json::to_string(&run_id).unwrap();
        let decoded: RunId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, run_id);
    }

    #[test]
    fn test_run_id_roundtrip_parse_display() {
        let run_id = RunId::new();
        let rendered = run_id.to_string();
        let reparsed = RunId::from_str(&rendered).unwrap();
        assert_eq!(reparsed, run_id);
    }

    #[test]
    fn external_delivery_run_id_is_stable_and_flow_scoped() {
        let mob_id = MobId::from("ops");
        let key = "schedule:one";
        let first = RunId::for_external_delivery(&mob_id, &FlowId::from("alpha"), key);
        assert_eq!(
            first,
            RunId::for_external_delivery(&mob_id, &FlowId::from("alpha"), key)
        );
        assert_ne!(
            first,
            RunId::for_external_delivery(&mob_id, &FlowId::from("beta"), key)
        );
    }

    #[test]
    fn external_delivery_run_id_uses_an_unambiguous_typed_tuple() {
        let first = RunId::for_work_execution(&MobId::from("a"), &FlowId::from("b:flow:c"), "key");
        let second = RunId::for_work_execution(&MobId::from("a:flow:b"), &FlowId::from("c"), "key");
        assert_ne!(first, second);
    }

    #[test]
    fn test_flow_id_roundtrip_json() {
        let id = FlowId::from("flow-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FlowId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_step_id_roundtrip_json() {
        let id = StepId::from("step-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: StepId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_branch_id_roundtrip_json() {
        let id = BranchId::from("branch-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: BranchId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_frame_id_roundtrip_json() {
        let id = FrameId::from("frame-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FrameId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_loop_instance_id_roundtrip_json() {
        let id = LoopInstanceId::from("loop-instance-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: LoopInstanceId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_flow_node_id_roundtrip_json() {
        let id = FlowNodeId::from("node-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FlowNodeId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_loop_id_roundtrip_json() {
        let id = LoopId::from("loop-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: LoopId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_existing_ids_roundtrip() {
        let mob = MobId::from("mob-a");
        let profile = ProfileName::from("lead");
        assert_eq!(
            serde_json::from_str::<MobId>(&serde_json::to_string(&mob).unwrap()).unwrap(),
            mob
        );
        assert_eq!(
            serde_json::from_str::<ProfileName>(&serde_json::to_string(&profile).unwrap()).unwrap(),
            profile
        );
    }

    // --- The one encoding of a name into a member id ---

    /// Every name round-trips through its member id, which is a member
    /// comms-name component; a plain member id is its own name.
    #[test]
    fn a_name_round_trips_through_its_member_id() {
        for name in [
            "team:alice",
            "rt:review:singleton:0",
            "with_underscore:and:colons",
            "user@host",
            "a.b:c",
            "9starts-with-digit",
            "\u{fc}n\u{ef}code:1",
            "mk--reserved",
            "",
        ] {
            let member_id = member_id_for_name(name);
            assert!(
                member_id.starts_with(ENCODED_MEMBER_ID_MARKER),
                "{name:?} is encoded: {member_id:?}"
            );
            assert!(
                is_member_comms_component(&member_id),
                "{member_id:?} is a comms-name component"
            );
            assert_eq!(name_for_member_id(&member_id), name, "{name:?}");
        }
        for plain in ["worker", "lead-agent", "_x", "a_b"] {
            assert_eq!(member_id_for_name(plain), plain);
            assert_eq!(name_for_member_id(plain), plain);
        }
        assert_eq!(member_id_for_name("team:alice"), "mk--team_calice");
        assert_eq!(member_id_for_name("a_b:c"), "mk--a__b_cc");
    }

    /// A member id that begins with the marker but is no escape production
    /// is its own name.
    #[test]
    fn a_member_id_that_is_no_encoding_is_its_own_name() {
        for member_id in ["mk--bad_q", "mk--open_x41", "mk--trailing_"] {
            assert_eq!(name_for_member_id(member_id), member_id);
        }
    }

    // --- Identity-first model types ---

    #[test]
    fn test_agent_identity_roundtrip_json() {
        let id = AgentIdentity::from("researcher");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, "\"researcher\"");
        let decoded: AgentIdentity = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_agent_identity_display() {
        let id = AgentIdentity::from("lead-agent");
        assert_eq!(id.to_string(), "lead-agent");
        assert_eq!(id.as_str(), "lead-agent");
    }

    #[test]
    fn test_generation_roundtrip_json() {
        let generation = Generation::new(42);
        let encoded = serde_json::to_string(&generation).unwrap();
        assert_eq!(encoded, "42");
        let decoded: Generation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, generation);
    }

    #[test]
    fn test_generation_initial_and_next() {
        assert_eq!(Generation::INITIAL.get(), 0);
        assert_eq!(Generation::INITIAL.next().expect("generation 1").get(), 1);
        assert_eq!(Generation::new(5).next().expect("generation 6").get(), 6);
        assert_eq!(Generation::new(u64::MAX).next(), None);
    }

    #[test]
    fn test_generation_ordering() {
        assert!(Generation::new(0) < Generation::new(1));
        assert!(Generation::new(1) < Generation::new(100));
    }

    #[test]
    fn test_agent_runtime_id_roundtrip_json() {
        let rid = AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(3));
        let encoded = serde_json::to_string(&rid).unwrap();
        let decoded: AgentRuntimeId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, rid);
    }

    #[test]
    fn test_agent_runtime_id_initial() {
        let rid = AgentRuntimeId::initial(AgentIdentity::from("worker"));
        assert_eq!(rid.identity, AgentIdentity::from("worker"));
        assert_eq!(rid.generation, Generation::INITIAL);
    }

    #[test]
    fn test_agent_runtime_id_display() {
        let rid = AgentRuntimeId::new(AgentIdentity::from("coder"), Generation::new(2));
        assert_eq!(rid.to_string(), "coder:2");
    }

    #[test]
    fn test_fence_token_roundtrip_json() {
        let ft = FenceToken::new(99);
        let encoded = serde_json::to_string(&ft).unwrap();
        assert_eq!(encoded, "99");
        let decoded: FenceToken = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, ft);
    }

    #[test]
    fn test_fence_token_display() {
        assert_eq!(FenceToken::new(7).to_string(), "fence:7");
    }

    #[test]
    fn test_fence_token_ordering() {
        assert!(FenceToken::new(1) < FenceToken::new(2));
    }

    #[test]
    fn test_work_ref_roundtrip_json() {
        let wr = WorkRef::new();
        let encoded = serde_json::to_string(&wr).unwrap();
        let decoded: WorkRef = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, wr);
    }

    #[test]
    fn test_work_ref_roundtrip_parse_display() {
        let wr = WorkRef::new();
        let rendered = wr.to_string();
        let reparsed = WorkRef::from_str(&rendered).unwrap();
        assert_eq!(reparsed, wr);
    }

    #[test]
    fn external_delivery_work_ref_is_stable_and_member_scoped() {
        let mob_id = MobId::from("ops");
        let key = "schedule:one";
        let first = WorkRef::for_external_delivery(&mob_id, &AgentIdentity::from("alpha"), key);
        assert_eq!(
            first,
            WorkRef::for_external_delivery(&mob_id, &AgentIdentity::from("alpha"), key)
        );
        assert_ne!(
            first,
            WorkRef::for_external_delivery(&mob_id, &AgentIdentity::from("beta"), key)
        );
    }

    #[test]
    fn test_work_spec_roundtrip_json() {
        let spec = WorkSpec::new("do something".to_owned(), WorkOrigin::External);
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: WorkSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.content,
            meerkat_core::types::ContentInput::from("do something".to_string()),
        );
        assert_eq!(decoded.origin, WorkOrigin::External);
    }

    #[test]
    fn test_work_origin_variants_roundtrip_json() {
        for origin in [WorkOrigin::External, WorkOrigin::Internal] {
            let encoded = serde_json::to_string(&origin).unwrap();
            let decoded: WorkOrigin = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, origin);
        }
    }

    #[test]
    fn test_work_spec_injected_context_serde_default_and_omission() {
        // Absent field defaults to empty (pre-field payloads stay readable).
        let decoded: WorkSpec =
            serde_json::from_str(r#"{"content":"do something","origin":"External"}"#).unwrap();
        assert!(decoded.injected_context.is_empty());

        // Empty is omitted; non-empty round-trips in order.
        let empty = WorkSpec::new("do something".to_owned(), WorkOrigin::External);
        let value = serde_json::to_value(&empty).unwrap();
        assert!(value.get("injected_context").is_none());

        let spec = WorkSpec::new("do something".to_owned(), WorkOrigin::External)
            .with_injected_context(vec![
                meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
            ]);
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: WorkSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.injected_context,
            vec![
                meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
            ],
        );
    }

    #[test]
    fn test_work_spec_internal_origin() {
        let spec = WorkSpec::new("coordinate".to_owned(), WorkOrigin::Internal);
        assert_eq!(spec.origin, WorkOrigin::Internal);
        assert_eq!(
            spec.content,
            meerkat_core::types::ContentInput::from("coordinate".to_string()),
        );
    }

    #[test]
    fn test_work_spec_accepts_multimodal_content() {
        // DELETE_ME C6 regression: WorkSpec.content must be ContentInput
        // (multimodal), not String. This test locks in that non-text
        // ContentInput variants (e.g. image blocks) can be submitted as
        // work content without string-coercing them first.
        let image_block = meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: meerkat_core::ImageData::Inline {
                data: "iVBORw0KGgo=".to_string(),
            },
        };
        let content = meerkat_core::types::ContentInput::Blocks(vec![
            meerkat_core::types::ContentBlock::Text {
                text: "analyse this".to_string(),
            },
            image_block.clone(),
        ]);
        let spec = WorkSpec::new(content.clone(), WorkOrigin::External);
        assert_eq!(spec.content, content);
    }
}
