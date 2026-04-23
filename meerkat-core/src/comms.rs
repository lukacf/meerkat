//! Canonical communication API types for Meerkat.
//!
//! This module defines the public contract for comms command, response, and stream
//! controls. It intentionally stays transport-agnostic and keeps names stable for
//! the host and SDK surface migration work.

use crate::event::{AgentEvent, EventEnvelope};
use crate::interaction::{InteractionId, ResponseStatus};
use crate::types::{ContentBlock, HandlingMode};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use uuid::Uuid;

/// Canonical runtime identity for a peer.
///
/// `PeerId` is the routing key: the router and trust store key by `PeerId`,
/// never by `PeerName`. Two peers may legitimately share a display `PeerName`
/// (per the Wave-B V5 dogma note), but their `PeerId`s never collide — the
/// underlying UUID is globally unique.
///
/// Constructed either freshly (`PeerId::new`) for a peer minted locally,
/// or parsed from a hyphenated UUID (`PeerId::parse`) when we've been given
/// an identity over the wire.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl PeerId {
    /// Mint a new `PeerId` with a fresh UUID v7 (time-ordered).
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing UUID.
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Parse a hyphenated UUID string into a `PeerId`.
    pub fn parse(s: &str) -> Result<Self, PeerIdError> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|source| PeerIdError::Invalid {
                input: s.to_string(),
                source,
            })
    }

    /// Hyphenated UUID string form.
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }

    /// Borrow the underlying UUID.
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for PeerId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Error parsing a [`PeerId`] from a string.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PeerIdError {
    #[error("invalid peer id {input:?}: {source}")]
    Invalid {
        input: String,
        #[source]
        source: uuid::Error,
    },
}

/// Typed transport atom for a peer address.
///
/// Replaces the old free-form `address: String` on `PeerDirectoryEntry` so
/// callers cannot accidentally invent new transports by string concatenation
/// at a call site.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PeerTransport {
    /// In-process routing within this runtime (no network hop).
    Inproc,
    /// Unix domain socket.
    Uds,
    /// TCP endpoint.
    Tcp,
}

impl PeerTransport {
    /// Stable short code used as the URI scheme half of a peer address.
    pub const fn as_scheme(&self) -> &'static str {
        match self {
            Self::Inproc => "inproc",
            Self::Uds => "uds",
            Self::Tcp => "tcp",
        }
    }
}

impl std::fmt::Display for PeerTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_scheme())
    }
}

/// Typed peer address: transport atom plus endpoint string.
///
/// The `endpoint` is transport-specific (path for `Uds`, `host:port` for
/// `Tcp`, agent name for `Inproc`) but is carried as a validated `String`
/// so the transport atom can be branched on without re-parsing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerAddress {
    pub transport: PeerTransport,
    pub endpoint: String,
}

impl PeerAddress {
    pub fn new(transport: PeerTransport, endpoint: impl Into<String>) -> Self {
        Self {
            transport,
            endpoint: endpoint.into(),
        }
    }

    pub const fn transport(&self) -> PeerTransport {
        self.transport
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl std::fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.transport.as_scheme(), self.endpoint)
    }
}

/// Display-only slug for a peer.
///
/// `PeerName` is **not** a routing key after Wave-B V5: the router resolves
/// sends by [`PeerId`], and trust stores are keyed by [`PeerId`]. `PeerName`
/// is retained so human-facing surfaces (CLI, REST `comms.peers`, logs) can
/// render a recognisable handle next to the opaque id.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerName(String);

impl PeerName {
    /// Create a new peer name if it passes basic validation.
    pub fn new(name: impl Into<String>) -> Result<Self, String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("peer name cannot be empty".to_string());
        }
        if name.chars().any(char::is_control) {
            return Err("peer name cannot contain control characters".to_string());
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_string(&self) -> String {
        self.0.clone()
    }
}

impl AsRef<str> for PeerName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for PeerName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<PeerName> for String {
    fn from(peer_name: PeerName) -> Self {
        peer_name.0
    }
}

/// Routing-subset descriptor for a trusted peer — the identity fields that
/// traverse the core seam.
///
/// Replaces the old stringly trusted-peer spec `{ name, peer_id, address }`
/// with typed atoms: `PeerId` (runtime routing key), `PeerName` (display
/// slug), `PeerAddress` (transport + endpoint), and a 32-byte signing
/// public key that lets the receiver verify envelope signatures. Richer
/// trust-store metadata (reachability snapshots, discovery labels) stays
/// in `meerkat-comms::trust::TrustedPeer` — this descriptor is the
/// minimal typed subset the core seam needs to route and admit a peer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeerDescriptor {
    /// Canonical runtime identity — the routing key. Never collides.
    pub peer_id: PeerId,
    /// Display-only slug for humans. Two peers may legitimately share a
    /// name; their `peer_id` values still differ.
    pub name: PeerName,
    /// Typed transport atom + endpoint. Transport cannot be invented by
    /// string concatenation at a call site.
    pub address: PeerAddress,
    /// Ed25519 signing public key (32 bytes). The receiver needs this to
    /// verify envelope signatures; the router derives `PeerId` from it
    /// via UUIDv5 so `peer_id` and `pubkey` are consistent.
    pub pubkey: [u8; 32],
}

impl TrustedPeerDescriptor {
    /// Parse string-shaped identity fields into a typed descriptor with
    /// a **zero Ed25519 signing pubkey**.
    ///
    /// The zero-pubkey default is **test-only** — envelope signature
    /// verification trivially fails against it. In-process `inproc`
    /// tests use this shape because the router identity map is what
    /// authorizes the peer; production paths must construct
    /// `TrustedPeerDescriptor` via the struct literal with an explicit
    /// pubkey (or use [`Self::with_pubkey`] to stamp one onto a
    /// test-built descriptor). The loud name keeps the hazard surface
    /// explicit — a production call site using this helper is always
    /// wrong and will read wrong at review.
    pub fn test_only_unsigned(
        name: impl Into<String>,
        peer_id: impl AsRef<str>,
        address: impl AsRef<str>,
    ) -> Result<Self, String> {
        let name = PeerName::new(name).map_err(|e| format!("invalid peer name: {e}"))?;
        let peer_id =
            PeerId::parse(peer_id.as_ref()).map_err(|e| format!("invalid peer_id: {e}"))?;
        let address_raw = address.as_ref();
        let (scheme, endpoint) = address_raw
            .split_once("://")
            .ok_or_else(|| format!("peer address missing transport scheme: {address_raw}"))?;
        let transport = match scheme {
            "inproc" => PeerTransport::Inproc,
            "uds" => PeerTransport::Uds,
            "tcp" => PeerTransport::Tcp,
            other => return Err(format!("unknown peer address transport: {other}")),
        };
        Ok(Self {
            peer_id,
            name,
            address: PeerAddress::new(transport, endpoint),
            pubkey: [0u8; 32],
        })
    }

    /// Attach a non-zero Ed25519 signing pubkey. Test and production
    /// paths that already have a derived `PeerId` + pubkey use the
    /// field-literal constructor directly; this helper is for
    /// retroactively stamping a pubkey onto a descriptor built via
    /// [`Self::test_only_unsigned`].
    pub fn with_pubkey(mut self, pubkey: [u8; 32]) -> Self {
        self.pubkey = pubkey;
        self
    }
}

/// One-way peer lifecycle notification kind.
///
/// These notifications are control-plane topology updates, not correlated
/// peer work requests. They intentionally do not create request/response
/// lifecycles and must never require an LLM-authored reply.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PeerLifecycleKind {
    #[serde(rename = "mob.peer_added")]
    PeerAdded,
    #[serde(rename = "mob.peer_retired")]
    PeerRetired,
    #[serde(rename = "mob.peer_unwired")]
    PeerUnwired,
}

impl PeerLifecycleKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PeerAdded => "mob.peer_added",
            Self::PeerRetired => "mob.peer_retired",
            Self::PeerUnwired => "mob.peer_unwired",
        }
    }
}

impl std::fmt::Display for PeerLifecycleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed wire request for `comms/send`.
///
/// Variants are serde-tagged on `kind` and validated structurally at the
/// deserialization boundary. Required fields per kind are enforced by the
/// type system; invalid discriminators (`source`, `stream`, `handling_mode`,
/// `status`) become serde deserialization errors rather than runtime
/// string-match failures.
///
/// Cross-field invariants that cannot be expressed structurally (e.g.
/// `handling_mode` is forbidden on `Accepted` peer responses) are checked
/// in [`CommsCommandRequest::into_command`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommsCommandRequest {
    /// Inject input into the local session.
    Input {
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<InputSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_self_session: Option<bool>,
    },
}

/// Cross-field validation failure for [`CommsCommandRequest::into_command`].
///
/// Per-field discriminator validation is enforced by serde at deserialization
/// — only invariants that span multiple fields surface here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommsCommandError {
    /// `handling_mode` is set on a `peer_response` whose `status` is
    /// `Accepted`. Progress responses cannot carry a handling mode — the
    /// receiver's admission gate would drop them, so reject at parse time.
    #[error("handling_mode is forbidden on accepted peer responses")]
    HandlingModeForbiddenForAcceptedResponse,
}

impl CommsCommandRequest {
    /// Convert the typed wire request into a [`CommsCommand`] domain envelope.
    ///
    /// `session_id` is supplied separately because it is owned by the
    /// surface that received the request, not the wire payload.
    pub fn into_command(
        self,
        session_id: &crate::types::SessionId,
    ) -> Result<CommsCommand, CommsCommandError> {
        Ok(match self {
            CommsCommandRequest::Input {
                body,
                blocks,
                source,
                stream,
                handling_mode,
                allow_self_session,
            } => CommsCommand::Input {
                session_id: session_id.clone(),
                body,
                blocks,
                handling_mode: handling_mode.unwrap_or_default(),
                source: source.unwrap_or(InputSource::Rpc),
                stream: stream.unwrap_or(InputStreamMode::None),
                allow_self_session: allow_self_session.unwrap_or(false),
            },
        })
    }

    /// Stable wire discriminant for telemetry / logging.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Input { .. } => "input",
        }
    }
}
/// Source for an input event posted to an agent.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSource {
    Tcp,
    Uds,
    Stdin,
    Webhook,
    Rpc,
}

impl From<crate::config::PlainEventSource> for InputSource {
    fn from(source: crate::config::PlainEventSource) -> Self {
        match source {
            crate::config::PlainEventSource::Tcp => Self::Tcp,
            crate::config::PlainEventSource::Uds => Self::Uds,
            crate::config::PlainEventSource::Stdin => Self::Stdin,
            crate::config::PlainEventSource::Webhook => Self::Webhook,
            crate::config::PlainEventSource::Rpc => Self::Rpc,
        }
    }
}

impl From<InputSource> for crate::config::PlainEventSource {
    fn from(source: InputSource) -> Self {
        match source {
            InputSource::Tcp => Self::Tcp,
            InputSource::Uds => Self::Uds,
            InputSource::Stdin => Self::Stdin,
            InputSource::Webhook => Self::Webhook,
            InputSource::Rpc => Self::Rpc,
        }
    }
}

/// Whether this input/peer command should reserve a local interaction stream.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputStreamMode {
    /// Do not reserve any stream.
    None,
    /// Reserve an interaction stream for the command.
    ReserveInteraction,
}

/// Transport-independent comms command envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommsCommand {
    /// Inject input into the local session.
    Input {
        session_id: crate::types::SessionId,
        body: String,
        blocks: Option<Vec<ContentBlock>>,
        handling_mode: HandlingMode,
        source: InputSource,
        stream: InputStreamMode,
        allow_self_session: bool,
    },
    /// Send a one-way peer message.
    PeerMessage {
        to: PeerName,
        body: String,
        blocks: Option<Vec<ContentBlock>>,
        handling_mode: HandlingMode,
    },
    /// Send a one-way peer lifecycle notification.
    PeerLifecycle {
        to: PeerName,
        kind: PeerLifecycleKind,
        params: serde_json::Value,
    },
    /// Send a request to a peer.
    PeerRequest {
        to: PeerName,
        intent: String,
        params: serde_json::Value,
        handling_mode: HandlingMode,
        stream: InputStreamMode,
    },
    /// Send a response to a prior peer request.
    PeerResponse {
        to: PeerName,
        in_reply_to: InteractionId,
        status: ResponseStatus,
        result: serde_json::Value,
        handling_mode: Option<HandlingMode>,
    },
}

impl CommsCommand {
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::Input { .. } => "input",
            Self::PeerMessage { .. } => "peer_message",
            Self::PeerLifecycle { .. } => "peer_lifecycle",
            Self::PeerRequest { .. } => "peer_request",
            Self::PeerResponse { .. } => "peer_response",
        }
    }
}

/// Receipt returned after accepting a comms command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendReceipt {
    InputAccepted {
        interaction_id: InteractionId,
        stream_reserved: bool,
    },
    PeerMessageSent {
        envelope_id: uuid::Uuid,
        acked: bool,
    },
    PeerLifecycleSent {
        envelope_id: uuid::Uuid,
    },
    PeerRequestSent {
        envelope_id: uuid::Uuid,
        interaction_id: InteractionId,
        stream_reserved: bool,
    },
    PeerResponseSent {
        envelope_id: uuid::Uuid,
        in_reply_to: InteractionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerDirectorySource {
    Trusted,
    Inproc,
    TrustedAndInproc,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerReachability {
    Unknown,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PeerReachabilityReason {
    OfflineOrNoAck,
    TransportError,
    /// The peer admitted the transport but rejected our envelope at its
    /// ingress policy gate (untrusted sender, full inbox, etc.). The peer
    /// is still reachable at the transport level; policy denied us.
    AdmissionDropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDirectoryEntry {
    /// Canonical runtime identity — the routing key.
    pub peer_id: PeerId,
    /// Display-only slug. Multiple entries may share a name; none share a
    /// `peer_id`.
    pub name: PeerName,
    /// Typed transport atom + endpoint. Replaces the prior free-form
    /// `address: String` so the transport cannot be invented by string
    /// concatenation at a call site.
    pub address: PeerAddress,
    pub source: PeerDirectorySource,
    pub sendable_kinds: Vec<String>,
    pub capabilities: serde_json::Value,
    pub reachability: PeerReachability,
    pub last_unreachable_reason: Option<PeerReachabilityReason>,
    /// Supplementary discovery metadata (description, labels).
    pub meta: crate::PeerMeta,
}

/// Scope for streaming event output.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StreamScope {
    Session(crate::types::SessionId),
    Interaction(InteractionId),
}

/// Typed stream over enveloped agent events.
pub type EventStream = Pin<Box<dyn Stream<Item = EventEnvelope<AgentEvent>> + Send>>;

/// Errors for stream attachment and lookup.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum StreamError {
    #[error("interaction not reserved: {0}")]
    NotReserved(InteractionId),
    #[error("stream not found: {0}")]
    NotFound(String),
    #[error("already attached: {0}")]
    AlreadyAttached(InteractionId),
    #[error("stream closed")]
    Closed,
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("internal: {0}")]
    Internal(String),
}

/// Typed reason a peer rejected our envelope at its ingress admission gate.
///
/// This mirrors `meerkat_comms::DropReason` across the core boundary so
/// `SendError::AdmissionDropped` can carry the typed cause all the way to
/// REST/RPC/MCP error payloads. Callers distinguish transport-level failure
/// (`PeerOffline`) from policy-level rejection (`AdmissionDropped { reason }`)
/// without collapsing both into "peer unreachable".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdmissionDropReason {
    /// `require_peer_auth` is on, the sender is not in the trusted set, and
    /// the envelope is not auth-exempt (e.g. supervisor-bridge bootstrap).
    UntrustedSender,
    /// Classification rejected the item before the admission gate ran.
    ClassificationRejected,
    /// The receiver's classified inbox is closed (receiver dropped).
    SessionClosed,
    /// The receiver's classified inbox is at capacity.
    InboxFull,
}

impl AdmissionDropReason {
    /// Stable wire code for this drop reason, suitable for REST/RPC/MCP
    /// error payloads. Callers-facing discriminant — must stay stable.
    pub fn as_code(&self) -> &'static str {
        match self {
            AdmissionDropReason::UntrustedSender => "untrusted_sender",
            AdmissionDropReason::ClassificationRejected => "classification_rejected",
            AdmissionDropReason::SessionClosed => "session_closed",
            AdmissionDropReason::InboxFull => "inbox_full",
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum SendError {
    #[error("peer not found: {0}")]
    PeerNotFound(String),
    #[error("peer offline")]
    PeerOffline,
    #[error("peer not sendable")]
    PeerNotSendable(String),
    #[error("input stream closed")]
    InputClosed,
    #[error("unsupported command: {0}")]
    Unsupported(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("internal: {0}")]
    Internal(String),
    /// Receiver admitted the envelope-transport but rejected it at ingress
    /// for a typed policy reason (untrusted sender, full inbox, etc.). This
    /// is semantically distinct from `PeerOffline` — transport worked,
    /// policy refused.
    #[error("peer dropped at admission: {reason:?}")]
    AdmissionDropped { reason: AdmissionDropReason },
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SendAndStreamError {
    #[error("send failed: {0}")]
    Send(#[from] SendError),
    #[error("stream attach failed: receipt={receipt:?}, error={error}")]
    StreamAttach {
        receipt: SendReceipt,
        error: StreamError,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn peer_name_validation() {
        assert!(PeerName::new("alice").is_ok());
        assert!(PeerName::new("".to_string()).is_err());
        assert!(PeerName::new("bad\x00name").is_err());
    }

    #[test]
    fn peer_directory_entry_fields() -> Result<(), String> {
        let entry = PeerDirectoryEntry {
            peer_id: PeerId::new(),
            name: PeerName::new("agent")?,
            address: PeerAddress::new(PeerTransport::Inproc, "agent"),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec!["peer_message".to_string()],
            capabilities: Value::Object(serde_json::Map::default()),
            reachability: PeerReachability::Unknown,
            last_unreachable_reason: None,
            meta: crate::PeerMeta::default(),
        };
        assert_eq!(entry.name.as_str(), "agent");
        assert_eq!(entry.address.transport(), PeerTransport::Inproc);
        assert_eq!(entry.address.endpoint(), "agent");
        assert_eq!(entry.source, PeerDirectorySource::Inproc);
        Ok(())
    }

    #[test]
    fn peer_id_parse_round_trip() {
        let id = PeerId::new();
        let parsed = PeerId::parse(&id.as_str()).expect("parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn peer_id_parse_rejects_garbage() {
        let err = PeerId::parse("not-a-uuid").expect_err("parse must reject");
        match err {
            PeerIdError::Invalid { input, .. } => assert_eq!(input, "not-a-uuid"),
        }
    }

    #[test]
    fn peer_address_display() {
        let addr = PeerAddress::new(PeerTransport::Tcp, "127.0.0.1:4200");
        assert_eq!(addr.to_string(), "tcp://127.0.0.1:4200");
    }

    #[test]
    fn input_stream_mode_roundtrip() -> Result<(), serde_json::Error> {
        let mode = InputStreamMode::ReserveInteraction;
        let serialized = serde_json::to_value(mode)?;
        assert_eq!(serialized.as_str(), Some("reserve_interaction"));
        assert_eq!(serde_json::from_value::<InputStreamMode>(serialized)?, mode);
        Ok(())
    }

    #[test]
    fn deserialize_input_with_typed_source() -> Result<(), serde_json::Error> {
        let json = r#"{"kind":"input","body":"hello","source":"webhook","handling_mode":"steer"}"#;
        let req: CommsCommandRequest = serde_json::from_str(json)?;
        match req {
            CommsCommandRequest::Input {
                body,
                source,
                handling_mode,
                ..
            } => {
                assert_eq!(body, "hello");
                assert_eq!(source, Some(InputSource::Webhook));
                assert_eq!(handling_mode, Some(HandlingMode::Steer));
            }
            other => panic!("expected Input, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn deserialize_input_invalid_source_rejects_at_serde_boundary() {
        let json = r#"{"kind":"input","body":"hello","source":"webhookd"}"#;
        let err = serde_json::from_str::<CommsCommandRequest>(json)
            .expect_err("invalid source must fail deserialization");
        let msg = err.to_string();
        // serde reports "unknown variant `webhookd`, expected one of ...".
        assert!(
            msg.contains("webhookd"),
            "error should name the rejected value, got: {msg}"
        );
    }

    #[test]
    fn deserialize_unknown_kind_rejects_at_serde_boundary() {
        let json = r#"{"kind":"foobar","body":"hello"}"#;
        let err = serde_json::from_str::<CommsCommandRequest>(json)
            .expect_err("unknown kind must fail deserialization");
        let msg = err.to_string();
        assert!(
            msg.contains("foobar") || msg.contains("variant"),
            "error should mention unknown variant, got: {msg}"
        );
    }
}
