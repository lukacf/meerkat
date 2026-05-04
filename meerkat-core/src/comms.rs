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
use std::collections::BTreeMap;
use std::pin::Pin;
use uuid::Uuid;

/// Comms request intent used for all supervisor bridge commands.
///
/// This is auth-exempt at peer ingress so a supervisor can complete the
/// bootstrap handshake before the private trust edge exists. Keep the literal
/// under core ingress authority; transport crates should compare through typed
/// core policy rather than owning a local string exemption.
pub const SUPERVISOR_BRIDGE_INTENT: &str = "supervisor.bridge";

/// Canonical runtime identity for a peer.
///
/// `PeerId` is the routing key: the router and trust store key by `PeerId`,
/// never by `PeerName`. Two peers may legitimately share a display `PeerName`
/// (per the Wave-B V5 dogma note), but their `PeerId`s never collide — the
/// underlying UUID is globally unique.
///
/// Constructed freshly (`PeerId::new`) for a peer minted locally, parsed
/// from a hyphenated UUID (`PeerId::parse`) when we've been given an identity
/// over the wire, or derived from a 32-byte Ed25519 public key when a transport
/// still authenticates by raw signing key.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

/// UUIDv5 namespace for deriving [`PeerId`] from an Ed25519 signing pubkey.
///
/// `PeerId` is the canonical runtime routing key: both the router and the
/// trust store index peers by `PeerId`, never by display name. The derivation
/// is a content hash of the 32-byte public key so a given key always resolves
/// to the same `PeerId` across runtimes.
const PEER_ID_ED25519_PUBKEY_NAMESPACE: Uuid =
    Uuid::from_u128(0x6d65_6572_6b61_7450_6565_7249_6430_0001);

impl PeerId {
    /// Mint a new `PeerId` with a fresh UUID v7 (time-ordered).
    pub fn new() -> Self {
        Self(crate::time_compat::new_uuid_v7())
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

    /// Derive the canonical routing id for a 32-byte Ed25519 public key.
    pub fn from_ed25519_pubkey(pubkey: &[u8; 32]) -> Self {
        Self(Uuid::new_v5(&PEER_ID_ED25519_PUBKEY_NAMESPACE, pubkey))
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

/// Error parsing a typed [`PeerAddress`] from its URI-shaped string form.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PeerAddressParseError {
    #[error("peer address missing transport scheme: {input}")]
    MissingTransportScheme { input: String },
    #[error("unknown peer address transport {scheme:?} in address {input:?}")]
    UnknownTransport { input: String, scheme: String },
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

    /// Strictly parse `scheme://endpoint` peer addresses.
    ///
    /// Only the currently supported transport schemes are accepted. Unknown
    /// schemes and schemeless input fail closed so callers cannot silently
    /// reinterpret address truth as TCP.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, PeerAddressParseError> {
        let raw = raw.as_ref();
        let (scheme, endpoint) =
            raw.split_once("://")
                .ok_or_else(|| PeerAddressParseError::MissingTransportScheme {
                    input: raw.to_string(),
                })?;
        let transport = match scheme {
            "inproc" => PeerTransport::Inproc,
            "uds" => PeerTransport::Uds,
            "tcp" => PeerTransport::Tcp,
            other => {
                return Err(PeerAddressParseError::UnknownTransport {
                    input: raw.to_string(),
                    scheme: other.to_string(),
                });
            }
        };
        Ok(Self::new(transport, endpoint))
    }
}

impl std::fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.transport.as_scheme(), self.endpoint)
    }
}

impl std::str::FromStr for PeerAddress {
    type Err = PeerAddressParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for PeerAddress {
    type Error = PeerAddressParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for PeerAddress {
    type Error = PeerAddressParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
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

/// Canonical outbound peer route.
///
/// `peer_id` is the only routing key. `display_name` is optional presentation
/// metadata retained for diagnostics after a boundary resolves a name through
/// trust or discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRoute {
    pub peer_id: PeerId,
    pub display_name: Option<PeerName>,
}

impl PeerRoute {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            display_name: None,
        }
    }

    pub fn with_display_name(peer_id: PeerId, display_name: PeerName) -> Self {
        Self {
            peer_id,
            display_name: Some(display_name),
        }
    }

    pub fn label(&self) -> String {
        self.display_name
            .as_ref()
            .map(PeerName::as_string)
            .unwrap_or_else(|| self.peer_id.to_string())
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
    pub fn pubkey_is_zero(pubkey: &[u8; 32]) -> bool {
        *pubkey == [0u8; 32]
    }

    pub fn has_zero_pubkey(&self) -> bool {
        Self::pubkey_is_zero(&self.pubkey)
    }

    pub fn validate_pubkey_for_peer_id(peer_id: PeerId, pubkey: &[u8; 32]) -> Result<(), String> {
        if Self::pubkey_is_zero(pubkey) {
            return Err("TrustedPeerDescriptor.pubkey must be non-zero".to_string());
        }
        let derived = PeerId::from_ed25519_pubkey(pubkey);
        if derived != peer_id {
            return Err(format!(
                "peer_id {peer_id} does not match pubkey-derived id {derived}"
            ));
        }
        Ok(())
    }

    /// Build a descriptor with a **zero Ed25519 signing pubkey** from
    /// typed identity atoms.
    ///
    /// The zero-pubkey default is **test-only** — envelope signature
    /// verification trivially fails against it. In-process `inproc`
    /// tests use this shape because the router identity map is what
    /// authorizes the peer; production paths construct
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
        let address = PeerAddress::parse(address.as_ref()).map_err(|e| e.to_string())?;
        Ok(Self {
            peer_id,
            name,
            address,
            pubkey: [0u8; 32],
        })
    }

    /// Typed sibling of [`Self::test_only_unsigned`]: build a descriptor
    /// from an already-typed [`PeerId`] instead of a stringly-typed peer-id
    /// argument.
    ///
    /// Post-#24 `PeerId` is a typed UUID; `PeerId::parse` only accepts
    /// hyphenated UUID strings. The stringly-typed
    /// [`Self::test_only_unsigned`] accepts anything `AsRef<str>` and
    /// round-trips through `PeerId::parse`, which is the right contract
    /// for call sites whose peer-id comes off the wire (comms-drain
    /// supervisor reconcile, ops lifecycle) — they receive a UUID string
    /// and the helper validates it.
    ///
    /// Test fixtures that mint a peer locally do NOT have a UUID string
    /// to start from. They have a debug-friendly alias (`"remote-agent-b"`,
    /// `"stale-peer"`) and want a random `PeerId`. The stringly form
    /// forced them to either (a) stamp the alias in as an invalid UUID
    /// (which rejects post-#24) or (b) reach outside the helper to mint
    /// a UUID separately. This typed sibling accepts the typed `PeerId`
    /// directly, skipping the parse round-trip.
    pub fn test_only_unsigned_typed(
        name: impl Into<String>,
        peer_id: PeerId,
        address: impl AsRef<str>,
    ) -> Result<Self, String> {
        let name = PeerName::new(name).map_err(|e| format!("invalid peer name: {e}"))?;
        let address = PeerAddress::parse(address.as_ref()).map_err(|e| e.to_string())?;
        Ok(Self {
            peer_id,
            name,
            address,
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

    /// Build a descriptor with a caller-supplied Ed25519 signing pubkey
    /// from typed identity atoms.
    ///
    /// This is the dogma-clean alternative to
    /// [`Self::test_only_unsigned`] for live-comms paths where the
    /// caller has a real pubkey (e.g. from
    /// `CommsRuntime::public_key().as_bytes()`). The supervisor needs
    /// a non-zero-pubkey trust entry for signed-envelope replies to
    /// admit past `is_trusted(&envelope.from)` at ingress.
    ///
    /// Like [`Self::test_only_unsigned`], this accepts a stringly
    /// `peer_id` that must parse as a UUID (post-#24 `PeerId::parse`
    /// only accepts hyphenated UUID strings). The hashed consistency
    /// check in [`crate::comms`] enforces that the supplied `peer_id`
    /// matches `PubKey::from(pubkey).to_peer_id()` at descriptor →
    /// trust conversion.
    pub fn unsigned_with_pubkey(
        name: impl Into<String>,
        peer_id: impl AsRef<str>,
        pubkey: [u8; 32],
        address: impl AsRef<str>,
    ) -> Result<Self, String> {
        let mut descriptor = Self::test_only_unsigned(name, peer_id, address)?;
        Self::validate_pubkey_for_peer_id(descriptor.peer_id, &pubkey)?;
        descriptor.pubkey = pubkey;
        Ok(descriptor)
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
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
    /// Send a one-way peer message.
    PeerMessage {
        to: PeerId,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
    /// Send a one-way peer lifecycle notification.
    PeerLifecycle {
        to: PeerId,
        lifecycle_kind: PeerLifecycleKind,
        #[serde(default)]
        params: serde_json::Value,
    },
    /// Send a request to a peer.
    PeerRequest {
        to: PeerId,
        intent: String,
        #[serde(default)]
        params: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
    },
    /// Send a response to a prior peer request.
    PeerResponse {
        to: PeerId,
        in_reply_to: InteractionId,
        status: ResponseStatus,
        #[serde(default)]
        result: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
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
            CommsCommandRequest::PeerMessage {
                to,
                body,
                blocks,
                handling_mode,
            } => CommsCommand::PeerMessage {
                to: PeerRoute::new(to),
                body,
                blocks,
                handling_mode: handling_mode.unwrap_or_default(),
            },
            CommsCommandRequest::PeerLifecycle {
                to,
                lifecycle_kind,
                params,
            } => CommsCommand::PeerLifecycle {
                to: PeerRoute::new(to),
                kind: lifecycle_kind,
                params,
            },
            CommsCommandRequest::PeerRequest {
                to,
                intent,
                params,
                handling_mode,
                stream,
            } => CommsCommand::PeerRequest {
                to: PeerRoute::new(to),
                intent,
                params,
                handling_mode: handling_mode.unwrap_or_default(),
                stream: stream.unwrap_or(InputStreamMode::None),
            },
            CommsCommandRequest::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                handling_mode,
            } => {
                if status == ResponseStatus::Accepted && handling_mode.is_some() {
                    return Err(CommsCommandError::HandlingModeForbiddenForAcceptedResponse);
                }
                CommsCommand::PeerResponse {
                    to: PeerRoute::new(to),
                    in_reply_to,
                    status,
                    result,
                    handling_mode,
                }
            }
        })
    }

    /// Stable wire discriminant for telemetry / logging.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Input { .. } => "input",
            Self::PeerMessage { .. } => "peer_message",
            Self::PeerLifecycle { .. } => "peer_lifecycle",
            Self::PeerRequest { .. } => "peer_request",
            Self::PeerResponse { .. } => "peer_response",
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
        to: PeerRoute,
        body: String,
        blocks: Option<Vec<ContentBlock>>,
        handling_mode: HandlingMode,
    },
    /// Send a one-way peer lifecycle notification.
    PeerLifecycle {
        to: PeerRoute,
        kind: PeerLifecycleKind,
        params: serde_json::Value,
    },
    /// Send a request to a peer.
    PeerRequest {
        to: PeerRoute,
        intent: String,
        params: serde_json::Value,
        handling_mode: HandlingMode,
        stream: InputStreamMode,
    },
    /// Send a response to a prior peer request.
    PeerResponse {
        to: PeerRoute,
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

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerDirectorySource {
    Trusted,
    Inproc,
    TrustedAndInproc,
    Unknown,
}

impl PeerDirectorySource {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Inproc => "inproc",
            Self::TrustedAndInproc => "trusted_and_inproc",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for PeerDirectorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerSendability {
    PeerMessage,
    PeerRequest,
    PeerResponse,
}

impl PeerSendability {
    pub const DIRECTORY_DEFAULTS: [Self; 3] =
        [Self::PeerMessage, Self::PeerRequest, Self::PeerResponse];

    pub fn directory_defaults() -> Vec<Self> {
        Self::DIRECTORY_DEFAULTS.to_vec()
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::PeerMessage => "peer_message",
            Self::PeerRequest => "peer_request",
            Self::PeerResponse => "peer_response",
        }
    }
}

impl std::fmt::Display for PeerSendability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed peer capability envelope for peer-directory output.
///
/// Extensions are intentionally opaque display/integration metadata. Core
/// routing, admission, and policy decisions must use typed fields such as
/// [`PeerDirectoryEntry::sendable_kinds`] instead of consulting this bag.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCapabilitySet {
    #[serde(default = "PeerCapabilitySet::default_version")]
    pub version: u16,
    #[serde(default)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl PeerCapabilitySet {
    pub const CURRENT_VERSION: u16 = 1;

    const fn default_version() -> u16 {
        Self::CURRENT_VERSION
    }

    pub fn with_extension(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extensions.insert(key.into(), value);
        self
    }
}

impl Default for PeerCapabilitySet {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            extensions: BTreeMap::new(),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerReachability {
    Unknown,
    Reachable,
    Unreachable,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub sendable_kinds: Vec<PeerSendability>,
    pub capabilities: PeerCapabilitySet,
    pub reachability: PeerReachability,
    pub last_unreachable_reason: Option<PeerReachabilityReason>,
    /// Supplementary discovery metadata (description, labels).
    pub meta: crate::PeerMeta,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDirectoryListing {
    pub peers: Vec<PeerDirectoryEntry>,
}

impl PeerDirectoryListing {
    pub fn new(peers: Vec<PeerDirectoryEntry>) -> Self {
        Self { peers }
    }
}

impl From<Vec<PeerDirectoryEntry>> for PeerDirectoryListing {
    fn from(peers: Vec<PeerDirectoryEntry>) -> Self {
        Self::new(peers)
    }
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
            sendable_kinds: vec![PeerSendability::PeerMessage],
            capabilities: PeerCapabilitySet::default(),
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
    fn peer_directory_listing_serializes_typed_source_sendability_and_capabilities()
    -> Result<(), String> {
        let entry = PeerDirectoryEntry {
            peer_id: PeerId::new(),
            name: PeerName::new("agent")?,
            address: PeerAddress::new(PeerTransport::Inproc, "agent"),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec![PeerSendability::PeerMessage, PeerSendability::PeerRequest],
            capabilities: PeerCapabilitySet::default()
                .with_extension("vendor.echo", serde_json::json!({ "enabled": true })),
            reachability: PeerReachability::Reachable,
            last_unreachable_reason: None,
            meta: crate::PeerMeta::default(),
        };

        let value = serde_json::to_value(PeerDirectoryListing::new(vec![entry]))
            .map_err(|err| err.to_string())?;
        let peer = &value["peers"][0];

        assert_eq!(peer["source"], "inproc");
        assert_eq!(
            peer["sendable_kinds"],
            serde_json::json!(["peer_message", "peer_request"])
        );
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(
            peer["capabilities"]["extensions"]["vendor.echo"]["enabled"],
            true
        );
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
    fn peer_address_parse_round_trips_supported_schemes() {
        let cases = [
            ("inproc://agent-a", PeerTransport::Inproc, "agent-a"),
            (
                "uds:///tmp/meerkat.sock",
                PeerTransport::Uds,
                "/tmp/meerkat.sock",
            ),
            ("tcp://127.0.0.1:4200", PeerTransport::Tcp, "127.0.0.1:4200"),
        ];

        for (raw, transport, endpoint) in cases {
            let parsed = PeerAddress::parse(raw).expect("supported address parses");
            assert_eq!(parsed.transport(), transport);
            assert_eq!(parsed.endpoint(), endpoint);
            assert_eq!(parsed.to_string(), raw);
        }
    }

    #[test]
    fn peer_address_parse_rejects_unknown_scheme() {
        let err = PeerAddress::parse("http://127.0.0.1:4200")
            .expect_err("unknown transport schemes must fail closed");
        assert!(
            err.to_string().contains("unknown peer address transport"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn peer_address_parse_rejects_schemeless_input() {
        let err = PeerAddress::parse("127.0.0.1:4200")
            .expect_err("strict parser requires an address scheme");
        assert!(
            err.to_string().contains("missing transport scheme"),
            "unexpected error: {err}",
        );
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
            other => panic!("expected input command request, got {other:?}"),
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
