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
        Self(Uuid::now_v7())
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

impl LogicalRuntimeId {
    const SESSION_RUNTIME_PREFIX: &'static str = "rt:session:";

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn for_session(session_id: &SessionId) -> Self {
        Self(format!("{}{session_id}", Self::SESSION_RUNTIME_PREFIX))
    }

    pub fn legacy_session_uuid_alias(session_id: &SessionId) -> Self {
        Self(session_id.to_string())
    }

    pub fn legacy_session_uuid_storage_alias(&self) -> Option<Self> {
        self.0
            .strip_prefix(Self::SESSION_RUNTIME_PREFIX)
            .map(Self::new)
    }

    pub fn storage_alias_candidates(&self) -> Vec<Self> {
        let Some(legacy) = self.legacy_session_uuid_storage_alias() else {
            return vec![self.clone()];
        };
        if legacy == *self {
            vec![self.clone()]
        } else {
            vec![self.clone(), legacy]
        }
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
        Self(Uuid::now_v7())
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
        Self(Uuid::now_v7())
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
        Self(Uuid::now_v7())
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

/// Version of the policy table used for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PolicyVersion(pub u64);

impl std::fmt::Display for PolicyVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Typed input-kind taxonomy used by the runtime policy table.
///
/// Every variant the policy table dispatches on is enumerated here. New input
/// kinds must be added to this enum AND wired into
/// `DefaultPolicyTable::resolve_by_kind`; the compiler enforces exhaustive
/// coverage.
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
