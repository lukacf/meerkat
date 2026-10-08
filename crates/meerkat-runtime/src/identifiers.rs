//! §6 Runtime-layer identifiers.
//!
//! These identifiers are used only by the runtime control-plane layer.
//! Core-facing identifiers (RunId, InputId) live in `meerkat-core::lifecycle`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
/// (`identity` at `generation`) does not, so durable deliveries addressed to a
/// member follow it across repoints and never reach a later respawn under a
/// new generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemberDeliveryAddress {
    pub mob_id: String,
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
    const MEMBER_RUNTIME_PREFIX: &'static str = "rt:member:";

    /// The delivery address of one member incarnation:
    /// `rt:member:{mob}:{identity}:{generation}`, with `%` and `:` escaped
    /// inside each component. Its own namespace, never read as a session.
    pub fn for_member(address: &MemberDeliveryAddress) -> Result<Self, LogicalRuntimeIdError> {
        if !valid_address_component(&address.mob_id) || !valid_address_component(&address.identity)
        {
            return Err(LogicalRuntimeIdError {
                id: format!("{address:?}"),
                reason: "member address components must be non-empty and free of control characters",
            });
        }
        Ok(Self(format!(
            "{}{}:{}:{}",
            Self::MEMBER_RUNTIME_PREFIX,
            encode_address_component(&address.mob_id),
            encode_address_component(&address.identity),
            address.generation
        )))
    }

    /// The member address this id names. `Ok(None)` for any id outside the
    /// `rt:member:` namespace; an id inside it that does not parse is an
    /// error, never a different kind of runtime.
    pub fn member_address(&self) -> Result<Option<MemberDeliveryAddress>, LogicalRuntimeIdError> {
        let Some(rest) = self.0.strip_prefix(Self::MEMBER_RUNTIME_PREFIX) else {
            return Ok(None);
        };
        let malformed = |reason| LogicalRuntimeIdError {
            id: self.0.clone(),
            reason,
        };
        let mut parts = rest.split(':');
        let (Some(mob_id), Some(identity), Some(generation), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(malformed(
                "expected rt:member:{mob}:{identity}:{generation}",
            ));
        };
        let mob_id = decode_address_component(mob_id)
            .ok_or_else(|| malformed("invalid escape in the mob component"))?;
        let identity = decode_address_component(identity)
            .ok_or_else(|| malformed("invalid escape in the identity component"))?;
        if !valid_address_component(&mob_id) || !valid_address_component(&identity) {
            return Err(malformed("empty or control-character component"));
        }
        let generation = generation
            .parse::<u64>()
            .map_err(|_| malformed("the generation is not a u64"))?;
        let address = MemberDeliveryAddress {
            mob_id,
            identity,
            generation,
        };
        // Round-trip: only the canonical spelling is a member address.
        if Self::for_member(&address)?.0 != self.0 {
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

    pub(crate) fn session_id(&self) -> Option<SessionId> {
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

    #[test]
    fn member_addresses_round_trip_with_escaped_components() {
        let address = MemberDeliveryAddress {
            mob_id: "team:a%b".into(),
            identity: "lead:1".into(),
            generation: 7,
        };
        let id = LogicalRuntimeId::for_member(&address).unwrap();
        assert_eq!(id.0, "rt:member:team%3Aa%25b:lead%3A1:7");
        assert_eq!(id.member_address().unwrap(), Some(address));
        assert_eq!(id.session_id(), None, "a member address is never a session");
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
        ] {
            assert!(
                LogicalRuntimeId::new(raw).member_address().is_err(),
                "{raw} must not parse as a member address"
            );
        }
        assert!(
            LogicalRuntimeId::for_member(&MemberDeliveryAddress {
                mob_id: String::new(),
                identity: "lead".into(),
                generation: 1,
            })
            .is_err()
        );
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
