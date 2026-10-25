//! §6 Runtime-layer identifiers.
//!
//! These identifiers are used only by the runtime control-plane layer.
//! Core-facing identifiers (RunId, InputId) live in `meerkat-core::lifecycle`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use meerkat_core::service::MobIncarnationId;
use meerkat_core::types::SessionId;

/// Unique identifier for a runtime event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeEventId(pub Uuid);

impl RuntimeEventId {
    pub fn new() -> Self {
        Self(meerkat_core::time_compat::new_uuid_v7())
    }
}

impl Default for RuntimeEventId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RuntimeEventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Logical identity of a runtime instance (survives retire/recycle).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogicalRuntimeId(pub String);

/// The stable delivery address of one mob member incarnation.
///
/// A member's sessions change when it is repointed, but its incarnation
/// (`identity` at `generation`, in one incarnation of its mob) does not, so
/// durable deliveries addressed to a member follow it across repoints and
/// never reach a later respawn under a new generation, nor the same identity
/// in a mob destroyed and created again under the same id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemberDeliveryAddress {
    pub mob_id: String,
    /// The incarnation of the mob the member belongs to. Every address
    /// minted since 0.9.4 names it. `None` is an address minted before:
    /// only the incarnation a mob created before 0.9.4 got at its first
    /// resume serves it. Only [`LogicalRuntimeId::member_address`] reads
    /// `None`; [`LogicalRuntimeId::for_member`] refuses it.
    pub incarnation: Option<MobIncarnationId>,
    pub identity: String,
    pub generation: u64,
}

/// A logical runtime id that claims a namespace it does not satisfy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid logical runtime id {id}: {reason}")]
pub struct LogicalRuntimeIdError {
    pub id: String,
    pub reason: &'static str,
}

fn encode_address_component(component: &str) -> String {
    component.replace('%', "%25").replace(':', "%3A")
}

fn decode_address_component(component: &str) -> Option<String> {
    let mut decoded = String::with_capacity(component.len());
    let mut rest = component;
    while let Some(at) = rest.find('%') {
        decoded.push_str(&rest[..at]);
        match rest.get(at..at + 3) {
            Some("%25") => decoded.push('%'),
            Some("%3A") => decoded.push(':'),
            _ => return None,
        }
        rest = &rest[at + 3..];
    }
    decoded.push_str(rest);
    Some(decoded)
}

fn valid_address_component(component: &str) -> bool {
    !component.is_empty() && !component.chars().any(char::is_control)
}

impl LogicalRuntimeId {
    const SESSION_RUNTIME_PREFIX: &'static str = "rt:session:";
    const MOB_MEMBER_RUNTIME_PREFIX: &'static str = "rt:mob-member:";
    const MEMBER_RUNTIME_PREFIX: &'static str = "rt:member:";

    /// The delivery address of one member incarnation:
    /// `rt:mob-member:{mob}:{incarnation}:{identity}:{generation}`, with `%`
    /// and `:` escaped inside the mob and identity components. Its own
    /// namespace, never read as a session. An address without a mob
    /// incarnation is refused: that form
    /// (`rt:member:{mob}:{identity}:{generation}`) is only read, from rows
    /// minted before 0.9.4, and a mob created since never serves it.
    pub fn for_member(address: &MemberDeliveryAddress) -> Result<Self, LogicalRuntimeIdError> {
        if address.incarnation.is_none() {
            return Err(LogicalRuntimeIdError {
                id: format!("{address:?}"),
                reason: "a member address without a mob incarnation is read-only (minted before 0.9.4)",
            });
        }
        Self::member_spelling(address).map(Self)
    }

    /// The canonical spelling of a member address in either form.
    fn member_spelling(address: &MemberDeliveryAddress) -> Result<String, LogicalRuntimeIdError> {
        if !valid_address_component(&address.mob_id) || !valid_address_component(&address.identity)
        {
            return Err(LogicalRuntimeIdError {
                id: format!("{address:?}"),
                reason: "member address components must be non-empty and free of control characters",
            });
        }
        let mob_id = encode_address_component(&address.mob_id);
        let identity = encode_address_component(&address.identity);
        let generation = address.generation;
        Ok(match address.incarnation {
            Some(incarnation) => format!(
                "{}{mob_id}:{incarnation}:{identity}:{generation}",
                Self::MOB_MEMBER_RUNTIME_PREFIX
            ),
            None => format!(
                "{}{mob_id}:{identity}:{generation}",
                Self::MEMBER_RUNTIME_PREFIX
            ),
        })
    }

    /// The member address this id names. `Ok(None)` for any id outside the
    /// `rt:mob-member:` and `rt:member:` namespaces; an id inside one that
    /// does not parse is an error, never a different kind of runtime.
    pub fn member_address(&self) -> Result<Option<MemberDeliveryAddress>, LogicalRuntimeIdError> {
        let (rest, names_incarnation) =
            if let Some(rest) = self.0.strip_prefix(Self::MOB_MEMBER_RUNTIME_PREFIX) {
                (rest, true)
            } else if let Some(rest) = self.0.strip_prefix(Self::MEMBER_RUNTIME_PREFIX) {
                (rest, false)
            } else {
                return Ok(None);
            };
        let malformed = |reason| LogicalRuntimeIdError {
            id: self.0.clone(),
            reason,
        };
        let mut parts = rest.split(':');
        let mob_id = parts.next();
        let incarnation = if names_incarnation {
            parts.next().map(Some)
        } else {
            Some(None)
        };
        let (Some(mob_id), Some(incarnation), Some(identity), Some(generation), None) = (
            mob_id,
            incarnation,
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(malformed(if names_incarnation {
                "expected rt:mob-member:{mob}:{incarnation}:{identity}:{generation}"
            } else {
                "expected rt:member:{mob}:{identity}:{generation}"
            }));
        };
        let mob_id = decode_address_component(mob_id)
            .ok_or_else(|| malformed("invalid escape in the mob component"))?;
        let identity = decode_address_component(identity)
            .ok_or_else(|| malformed("invalid escape in the identity component"))?;
        if !valid_address_component(&mob_id) || !valid_address_component(&identity) {
            return Err(malformed("empty or control-character component"));
        }
        let incarnation = incarnation
            .map(str::parse::<MobIncarnationId>)
            .transpose()
            .map_err(|_| malformed("the incarnation is not a mob incarnation id"))?;
        let generation = generation
            .parse::<u64>()
            .map_err(|_| malformed("the generation is not a u64"))?;
        let address = MemberDeliveryAddress {
            mob_id,
            incarnation,
            identity,
            generation,
        };
        // Round-trip: only the canonical spelling is a member address.
        if Self::member_spelling(&address)? != self.0 {
            return Err(malformed("not the canonical member address spelling"));
        }
        Ok(Some(address))
    }

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn for_session(session_id: &SessionId) -> Self {
        Self(format!("{}{session_id}", Self::SESSION_RUNTIME_PREFIX))
    }

    pub fn legacy_session_uuid_alias(session_id: &SessionId) -> Self {
        Self(session_id.to_string())
    }

    /// The session a session runtime's id names (`for_session`, or the
    /// legacy bare session id); `None` for any other runtime id.
    pub fn session_id(&self) -> Option<SessionId> {
        let raw = self
            .0
            .strip_prefix(Self::SESSION_RUNTIME_PREFIX)
            .unwrap_or(self.0.as_str());
        SessionId::parse(raw).ok()
    }
}

impl std::fmt::Display for LogicalRuntimeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifier for a conversation within a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub Uuid);

impl ConversationId {
    pub fn new() -> Self {
        Self(meerkat_core::time_compat::new_uuid_v7())
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifier linking an event to its cause.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CausationId(pub Uuid);

impl Default for CausationId {
    fn default() -> Self {
        Self::new()
    }
}

impl CausationId {
    pub fn new() -> Self {
        Self(meerkat_core::time_compat::new_uuid_v7())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl std::fmt::Display for CausationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Correlation identifier for tracing related events across boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationId(pub Uuid);

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationId {
    pub fn new() -> Self {
        Self(meerkat_core::time_compat::new_uuid_v7())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl std::fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Client-provided key for idempotent input submission.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(pub String);

impl IdempotencyKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// A caller-supplied key for a session input: checked by the canonical
    /// key rule ([`validate_canonical_key_text`], at most
    /// [`CALLER_IDEMPOTENCY_KEY_MAX_BYTES`]) and stored under
    /// [`CALLER_IDEMPOTENCY_KEY_NAMESPACE`], so it never meets a key Core
    /// derives for its own deliveries in the same session's input index.
    ///
    /// # Errors
    /// [`CanonicalKeyTextError`] when `raw` is not canonical key text.
    pub fn caller(raw: &str) -> Result<Self, CanonicalKeyTextError> {
        validate_canonical_key_text("idempotency_key", raw, CALLER_IDEMPOTENCY_KEY_MAX_BYTES)?;
        Ok(Self(format!("{CALLER_IDEMPOTENCY_KEY_NAMESPACE}{raw}")))
    }
}

/// Longest caller-supplied idempotency key, in bytes, before its namespace.
pub const CALLER_IDEMPOTENCY_KEY_MAX_BYTES: usize = 256;

/// The namespace a caller-supplied key is stored under in a session's input
/// index. Core's own keys (job deliveries, continuation admissions, peer
/// inputs) never start with it.
pub const CALLER_IDEMPOTENCY_KEY_NAMESPACE: &str = "caller:";

/// A key that is not canonical key text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field} must be canonical nonempty text no longer than {max_bytes} bytes")]
pub struct CanonicalKeyTextError {
    /// The field the key was supplied as, as the refusal names it.
    pub field: String,
    /// The cap the key exceeded or would have to meet.
    pub max_bytes: usize,
}

/// The one canonical key-text rule, shared by every caller-supplied key:
/// non-empty, no leading or trailing whitespace, at most `max_bytes` bytes,
/// and no control characters.
///
/// # Errors
/// [`CanonicalKeyTextError`] naming `field` when `value` breaks the rule.
pub fn validate_canonical_key_text(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), CanonicalKeyTextError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > max_bytes
        || value.chars().any(char::is_control)
    {
        return Err(CanonicalKeyTextError {
            field: field.to_string(),
            max_bytes,
        });
    }
    Ok(())
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Key for supersession scoping (same key = same supersession window).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupersessionKey(pub String);

impl SupersessionKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

impl std::fmt::Display for SupersessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Version of the generated admission policy used for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PolicyVersion(pub u64);

impl std::fmt::Display for PolicyVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Typed input-kind taxonomy used by the generated admission-policy projection.
///
/// Every variant the generated admission authority dispatches on is enumerated
/// here so compatibility projections can request policy decisions by typed
/// kind instead of by string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputKind {
    /// Operator/user prompt.
    Prompt,
    /// Peer message convention (or unconvented peer input).
    PeerMessage,
    /// Peer request convention.
    PeerRequest,
    /// Peer response progress convention.
    PeerResponseProgress,
    /// Peer response terminal convention.
    PeerResponseTerminal,
    /// Flow step input.
    FlowStep,
    /// External event input.
    ExternalEvent,
    /// Explicit continuation input.
    Continuation,
    /// Explicit operation/lifecycle input.
    Operation,
}

impl InputKind {
    /// Stable lowercase identifier. Wire formats and trace strings rely on
    /// this exact spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            InputKind::Prompt => "prompt",
            InputKind::PeerMessage => "peer_message",
            InputKind::PeerRequest => "peer_request",
            InputKind::PeerResponseProgress => "peer_response_progress",
            InputKind::PeerResponseTerminal => "peer_response_terminal",
            InputKind::FlowStep => "flow_step",
            InputKind::ExternalEvent => "external_event",
            InputKind::Continuation => "continuation",
            InputKind::Operation => "operation",
        }
    }
}

impl std::fmt::Display for InputKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Identifier for an input kind, wrapping the typed [`InputKind`] taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KindId(pub InputKind);

impl KindId {
    pub const fn new(kind: InputKind) -> Self {
        Self(kind)
    }

    pub const fn kind(self) -> InputKind {
        self.0
    }
}

impl From<InputKind> for KindId {
    fn from(kind: InputKind) -> Self {
        Self(kind)
    }
}

impl std::fmt::Display for KindId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Identifier for a schema definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaId(pub String);

impl SchemaId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for SchemaId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifier for a projection rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectionRuleId(pub String);

impl ProjectionRuleId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ProjectionRuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable event code for wire formats and SDK consumers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventCodeId(pub String);

impl EventCodeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for EventCodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const INCARNATION: &str = "0f6a3c2e-1b7d-4e59-8a10-2c4b6d8e9f01";

    #[test]
    fn member_addresses_round_trip_with_escaped_components() {
        let address = MemberDeliveryAddress {
            mob_id: "team:a%b".into(),
            incarnation: Some(INCARNATION.parse().unwrap()),
            identity: "lead:1".into(),
            generation: 7,
        };
        let id = LogicalRuntimeId::for_member(&address).unwrap();
        assert_eq!(
            id.0,
            format!("rt:mob-member:team%3Aa%25b:{INCARNATION}:lead%3A1:7")
        );
        assert_eq!(id.member_address().unwrap(), Some(address));
        assert_eq!(id.session_id(), None, "a member address is never a session");
    }

    /// An address minted before 0.9.4 names no mob incarnation. It still
    /// reads as the member address it was, and nothing mints that form.
    #[test]
    fn a_member_address_without_an_incarnation_is_read_only() {
        let address = MemberDeliveryAddress {
            mob_id: "team:a%b".into(),
            incarnation: None,
            identity: "lead:1".into(),
            generation: 7,
        };
        let id = LogicalRuntimeId::new("rt:member:team%3Aa%25b:lead%3A1:7");
        assert_eq!(id.member_address().unwrap(), Some(address.clone()));
        assert_eq!(id.session_id(), None, "a member address is never a session");
        assert!(
            LogicalRuntimeId::for_member(&address).is_err(),
            "the form without a mob incarnation is never minted"
        );
    }

    #[test]
    fn session_ids_are_outside_the_member_namespace() {
        let session = SessionId::new();
        let id = LogicalRuntimeId::for_session(&session);
        assert_eq!(id.member_address().unwrap(), None);
        assert_eq!(id.session_id(), Some(session));
    }

    #[test]
    fn a_malformed_member_address_is_an_error_not_another_runtime() {
        for raw in [
            "rt:member:",
            "rt:member:team:lead",
            "rt:member:team:lead:7:extra",
            "rt:member:team:lead:seven",
            "rt:member::lead:7",
            "rt:member:team%3:lead:7",
            "rt:member:team:lead:007",
            "rt:mob-member:",
            "rt:mob-member:team:lead:7",
            "rt:mob-member:team:not-an-incarnation:lead:7",
        ] {
            assert!(
                LogicalRuntimeId::new(raw).member_address().is_err(),
                "{raw} must not parse as a member address"
            );
        }
        for raw in [
            format!("rt:mob-member:team:{INCARNATION}:lead"),
            format!("rt:mob-member:team:{INCARNATION}:lead:7:extra"),
            format!("rt:mob-member::{INCARNATION}:lead:7"),
            format!("rt:mob-member:team:{INCARNATION}::7"),
            format!("rt:mob-member:team:{INCARNATION}:lead:seven"),
            format!("rt:mob-member:team:{INCARNATION}:lead:007"),
            format!("rt:mob-member:team:{}:lead:7", INCARNATION.to_uppercase()),
            format!("rt:mob-member:team:{}:lead:7", INCARNATION.replace('-', "")),
        ] {
            assert!(
                LogicalRuntimeId::new(raw.clone()).member_address().is_err(),
                "{raw} must not parse as a member address"
            );
        }
        for incarnation in [None, Some(INCARNATION.parse().unwrap())] {
            assert!(
                LogicalRuntimeId::for_member(&MemberDeliveryAddress {
                    mob_id: String::new(),
                    incarnation,
                    identity: "lead".into(),
                    generation: 1,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn runtime_event_id_unique() {
        let a = RuntimeEventId::new();
        let b = RuntimeEventId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn runtime_event_id_serde() {
        let id = RuntimeEventId::new();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: RuntimeEventId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn logical_runtime_id_serde() {
        let id = LogicalRuntimeId::new("agent-1");
        let json = serde_json::to_string(&id).unwrap();
        let parsed: LogicalRuntimeId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
        assert_eq!(id.to_string(), "agent-1");
    }

    #[test]
    fn conversation_id_unique() {
        let a = ConversationId::new();
        let b = ConversationId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn idempotency_key_serde() {
        let key = IdempotencyKey::new("req-abc-123");
        let json = serde_json::to_string(&key).unwrap();
        let parsed: IdempotencyKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, parsed);
    }

    #[test]
    fn supersession_key_serde() {
        let key = SupersessionKey::new("peer-status");
        let json = serde_json::to_string(&key).unwrap();
        let parsed: SupersessionKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, parsed);
    }

    #[test]
    fn policy_version_serde() {
        let v = PolicyVersion(42);
        let json = serde_json::to_string(&v).unwrap();
        let parsed: PolicyVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(v, parsed);
    }

    #[test]
    fn kind_id_display() {
        let id = KindId::new(InputKind::Prompt);
        assert_eq!(id.to_string(), "prompt");
        assert_eq!(
            KindId::new(InputKind::PeerResponseProgress).to_string(),
            "peer_response_progress"
        );
    }

    #[test]
    fn kind_id_serde_roundtrips_typed_variant() {
        let id = KindId::new(InputKind::PeerResponseTerminal);
        let json = serde_json::to_string(&id).unwrap();
        let parsed: KindId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }
}
