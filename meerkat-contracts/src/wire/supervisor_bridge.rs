//! Supervisor bridge protocol wire types.
//!
//! Typed wire envelope for cross-machine bridge commands between a mob
//! supervisor and the runtime instances it manages. Both `meerkat-mob`
//! (sender) and `meerkat-runtime` (receiver) consume these types. Neither
//! crate depends on the other — the contracts crate owns the vocabulary.

use meerkat_core::comms::{PeerAddress, PeerId, PeerName, TrustedPeerDescriptor};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// Comms intent used for all supervisor bridge commands.
///
/// The sender sets this as the request `intent`; the receiver checks for it
/// before attempting to deserialize `params` as [`BridgeCommand`].
pub use meerkat_core::comms::SUPERVISOR_BRIDGE_INTENT;
/// Address query parameter carrying the one-time bind bootstrap token.
pub const SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM: &str = "mob_supervisor_bootstrap_token";
/// A supported supervisor bridge wire protocol version.
///
/// The JSON representation remains the historic integer so persisted records
/// and wire payloads do not change shape. Construction is intentionally routed
/// through this type so unsupported values fail at serde/TryFrom boundaries
/// instead of being carried deeper as raw integers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BridgeProtocolVersion(u32);

/// Unsupported supervisor bridge wire protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedBridgeProtocolVersion {
    raw: u32,
}

impl fmt::Display for UnsupportedBridgeProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
            self.raw,
            BridgeProtocolVersion::SUPPORTED,
            BridgeProtocolVersion::DEFAULT
        )
    }
}

impl std::error::Error for UnsupportedBridgeProtocolVersion {}

impl BridgeProtocolVersion {
    /// Protocol with typed rejection causes.
    pub const V2: Self = Self(2);
    /// Current protocol version implemented by this bridge contract.
    pub const CURRENT: Self = Self::V2;
    /// Default protocol version for new supervisor authority records.
    pub const DEFAULT: Self = Self::V2;
    /// Protocol versions accepted by this bridge contract.
    pub const SUPPORTED: &'static [Self] = &[Self::V2];

    pub const fn is_supported(self) -> bool {
        matches!(self.0, 2)
    }

    pub const fn same_protocol_as(self, other: Self) -> bool {
        self.0 == other.0
    }

    pub fn supported() -> &'static [Self] {
        Self::SUPPORTED
    }

    fn from_supported_u32(raw: u32) -> Result<Self, UnsupportedBridgeProtocolVersion> {
        match raw {
            2 => Ok(Self::V2),
            _ => Err(UnsupportedBridgeProtocolVersion { raw }),
        }
    }
}

impl Default for BridgeProtocolVersion {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Debug for BridgeProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for BridgeProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<u32> for BridgeProtocolVersion {
    type Error = UnsupportedBridgeProtocolVersion;

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        Self::from_supported_u32(raw)
    }
}

impl Serialize for BridgeProtocolVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for BridgeProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = u32::deserialize(deserializer)?;
        Self::from_supported_u32(raw).map_err(de::Error::custom)
    }
}

/// Current supervisor bridge wire protocol version.
///
/// Version history:
/// - `1`: initial protocol with untyped `BridgeReply::Rejected { reason: String }`.
/// - `2`: `BridgeReply::Rejected { cause: BridgeRejectionCause, reason: String }`
///   so callers branch on typed cause; runtime-side emitters pass typed
///   `BridgeReply` values through to the transport. Delivery rejections
///   carry typed `BridgeDeliveryRejectionCause` data; the string `reason`
///   remains diagnostic presentation only.
pub const SUPERVISOR_BRIDGE_PROTOCOL_VERSION: BridgeProtocolVersion =
    BridgeProtocolVersion::CURRENT;
/// Canonical current supervisor bridge protocol version.
pub const SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION: BridgeProtocolVersion =
    BridgeProtocolVersion::CURRENT;
/// Canonical default supervisor bridge protocol version for new authorities.
pub const SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION: BridgeProtocolVersion =
    BridgeProtocolVersion::DEFAULT;
/// Canonical set of protocol versions this runtime/wire contract accepts.
pub const SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS: &[BridgeProtocolVersion] =
    BridgeProtocolVersion::SUPPORTED;

/// Return the canonical current supervisor bridge protocol version.
pub const fn supervisor_bridge_current_protocol_version() -> BridgeProtocolVersion {
    SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION
}

/// Return the canonical default supervisor bridge protocol version.
pub const fn supervisor_bridge_default_protocol_version() -> BridgeProtocolVersion {
    SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION
}

/// Return the canonical list of supported supervisor bridge protocol versions.
pub fn supervisor_bridge_supported_protocol_versions() -> &'static [BridgeProtocolVersion] {
    SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS
}

/// Return `true` when `protocol_version` is accepted by this bridge contract.
pub fn supervisor_bridge_protocol_version_supported(
    protocol_version: BridgeProtocolVersion,
) -> bool {
    protocol_version.is_supported()
}

fn default_supported_protocol_versions() -> Vec<BridgeProtocolVersion> {
    supervisor_bridge_supported_protocol_versions().to_vec()
}

/// Remove the one-time bind bootstrap token from an advertised bridge address.
pub fn canonicalize_bridge_address(address: &str) -> String {
    let Some((base, query)) = address.split_once('?') else {
        return address.to_string();
    };
    let filtered: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            pair.split_once('=')
                .map(|(key, _)| key != SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM)
                .unwrap_or(true)
        })
        .filter(|pair| !pair.is_empty())
        .collect();
    if filtered.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", filtered.join("&"))
    }
}

// ---------------------------------------------------------------------------
// Command envelope
// ---------------------------------------------------------------------------

/// A typed command sent from a supervisor to a member runtime.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeCommand {
    BindMember(BridgeBindPayload),
    AuthorizeSupervisor(BridgeSupervisorPayload),
    RevokeSupervisor(BridgeSupervisorPayload),
    DeliverMemberInput(BridgeDeliveryPayload),
    ObserveMember(BridgeSupervisorPayload),
    InterruptMember(BridgeSupervisorPayload),
    HardCancelMember(BridgeHardCancelPayload),
    RetireMember(BridgeSupervisorPayload),
    DestroyMember(BridgeSupervisorPayload),
    WireMember(BridgePeerWiringPayload),
    UnwireMember(BridgePeerWiringPayload),
}

impl BridgeCommand {
    /// Protocol version carried by this command's payload.
    pub fn protocol_version(&self) -> BridgeProtocolVersion {
        match self {
            Self::BindMember(payload) => payload.protocol_version,
            Self::AuthorizeSupervisor(payload)
            | Self::RevokeSupervisor(payload)
            | Self::ObserveMember(payload)
            | Self::InterruptMember(payload)
            | Self::RetireMember(payload)
            | Self::DestroyMember(payload) => payload.protocol_version,
            Self::HardCancelMember(payload) => payload.protocol_version,
            Self::DeliverMemberInput(payload) => payload.protocol_version,
            Self::WireMember(payload) | Self::UnwireMember(payload) => payload.protocol_version,
        }
    }
}

/// Decode failure for a supervisor bridge command.
#[derive(Debug)]
pub enum BridgeCommandDecodeError {
    UnsupportedProtocolVersion(UnsupportedBridgeProtocolVersion),
    Invalid(serde_json::Error),
}

impl fmt::Display for BridgeCommandDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion(error) => error.fmt(f),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BridgeCommandDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnsupportedProtocolVersion(error) => Some(error),
            Self::Invalid(error) => Some(error),
        }
    }
}

/// Decode a bridge command while preserving typed protocol-version failures.
pub fn decode_bridge_command(
    value: serde_json::Value,
) -> Result<BridgeCommand, BridgeCommandDecodeError> {
    if let Some(raw) = value
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
    {
        BridgeProtocolVersion::from_supported_u32(raw)
            .map_err(BridgeCommandDecodeError::UnsupportedProtocolVersion)?;
    }
    serde_json::from_value(value).map_err(BridgeCommandDecodeError::Invalid)
}

// ---------------------------------------------------------------------------
// Reply envelope
// ---------------------------------------------------------------------------

/// A typed reply from a member runtime back to the supervisor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeReply {
    BindMember(BridgeBindResponse),
    Ack(BridgeAck),
    Observation(BridgeObservationResponse),
    Delivery(BridgeDeliveryResponse),
    Retire(BridgeRetireResponse),
    Destroy(BridgeDestroyResponse),
    Rejected {
        cause: BridgeRejectionCause,
        reason: String,
    },
}

/// Decoded bridge rejection reply.
///
/// Protocol v2 rejections carry a typed [`BridgeRejectionCause`]. Bare JSON
/// string replies are only a protocol-v1 compatibility shape; callers must not
/// treat them as authoritative typed v2 causes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BridgeRejectionReply {
    Typed {
        cause: BridgeRejectionCause,
        reason: String,
    },
    LegacyV1RawString {
        reason: String,
    },
}

impl BridgeRejectionReply {
    pub fn reason(&self) -> &str {
        match self {
            Self::Typed { reason, .. } | Self::LegacyV1RawString { reason } => reason,
        }
    }

    pub fn typed_cause(&self) -> Option<BridgeRejectionCause> {
        match self {
            Self::Typed { cause, .. } => Some(*cause),
            Self::LegacyV1RawString { .. } => None,
        }
    }

    pub fn is_legacy_v1_raw_string(&self) -> bool {
        matches!(self, Self::LegacyV1RawString { .. })
    }
}

/// Decode a typed protocol-v2 bridge rejection.
///
/// Deliberately ignores bare JSON strings. Use
/// [`decode_legacy_v1_raw_string_rejection`] only when the command was sent
/// over the explicit protocol-v1 compatibility path.
pub fn decode_protocol_v2_bridge_rejection(
    value: &serde_json::Value,
) -> Option<BridgeRejectionReply> {
    match serde_json::from_value::<BridgeReply>(value.clone()).ok()? {
        BridgeReply::Rejected { cause, reason } => {
            Some(BridgeRejectionReply::Typed { cause, reason })
        }
        _ => None,
    }
}

/// Decode the legacy protocol-v1 raw-string rejection compatibility shape.
pub fn decode_legacy_v1_raw_string_rejection(
    value: &serde_json::Value,
) -> Option<BridgeRejectionReply> {
    value
        .as_str()
        .map(|reason| BridgeRejectionReply::LegacyV1RawString {
            reason: reason.to_string(),
        })
}

/// Decode a bridge rejection according to a supported command protocol version.
///
/// Supported supervisors expect typed rejection replies. The legacy v1 bare
/// string shape remains isolated in [`decode_legacy_v1_raw_string_rejection`]
/// and is never promoted through this supported-version path.
pub fn decode_bridge_rejection_reply(
    protocol_version: BridgeProtocolVersion,
    value: &serde_json::Value,
) -> Option<BridgeRejectionReply> {
    let _ = protocol_version;
    decode_protocol_v2_bridge_rejection(value)
}

/// Typed vocabulary for why a bridge command was rejected.
///
/// Callers branch on the typed `cause` to drive recovery logic; the
/// accompanying `reason` string is for operator diagnostics only and must
/// not be pattern-matched. Reserve `Internal` for true invariant
/// violations — ordinary validation failures get a specific cause.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeRejectionCause {
    /// No supervisor is bound yet; caller must `bind_member` first.
    NotBound,
    /// The request targets an older epoch than the bound authority.
    StaleSupervisor,
    /// The authenticated sender does not match the authorized supervisor.
    SenderMismatch,
    /// A different supervisor is already bound; rotation must go through
    /// `authorize_supervisor`, not `bind_member`.
    AlreadyBound,
    /// The bootstrap token did not match the runtime's expected value.
    InvalidBootstrapToken,
    /// The wire protocol version is not supported by this runtime.
    UnsupportedProtocolVersion,
    /// The embedded supervisor peer spec failed validation.
    InvalidSupervisorSpec,
    /// The embedded trusted-peer spec failed validation.
    InvalidPeerSpec,
    /// The `expected_address` in the bind payload does not match this
    /// runtime's advertised address.
    AddressMismatch,
    /// The command variant is not currently handled by this runtime.
    Unsupported,
    /// An unexpected invariant was violated while handling the command.
    Internal,
}

/// Recoverability class of a bridge rejection.
///
/// The class is a protocol-level property of each [`BridgeRejectionCause`]
/// variant — not a decision for downstream helpers to make by pattern
/// matching on a hardcoded cause set. Callers branch on the class to
/// decide whether recovery by re-running `BindMember` is appropriate.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeRejectionClass {
    /// Member is reachable and protocol-compliant, but its supervisor
    /// authority is missing or out-of-sync with the caller's. A fresh
    /// `BindMember` will reconcile.
    RecoverableBySupervisorRebind,
    /// The rejection reflects a hard contract violation (protocol
    /// version, identity, bootstrap proof, invariant) that a fresh bind
    /// cannot fix. The rejection must bubble up.
    Fatal,
}

impl BridgeRejectionCause {
    /// Protocol-level recoverability class for this rejection cause.
    pub const fn class(self) -> BridgeRejectionClass {
        match self {
            Self::NotBound | Self::StaleSupervisor | Self::SenderMismatch => {
                BridgeRejectionClass::RecoverableBySupervisorRebind
            }
            Self::AlreadyBound
            | Self::InvalidBootstrapToken
            | Self::UnsupportedProtocolVersion
            | Self::InvalidSupervisorSpec
            | Self::InvalidPeerSpec
            | Self::AddressMismatch
            | Self::Unsupported
            | Self::Internal => BridgeRejectionClass::Fatal,
        }
    }
}

// ---------------------------------------------------------------------------
// Member runtime state (wire projection)
// ---------------------------------------------------------------------------

/// Wire projection of a member's runtime state.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeMemberRuntimeState {
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

impl std::fmt::Display for BridgeMemberRuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initializing => write!(f, "initializing"),
            Self::Idle => write!(f, "idle"),
            Self::Attached => write!(f, "attached"),
            Self::Running => write!(f, "running"),
            Self::Retired => write!(f, "retired"),
            Self::Stopped => write!(f, "stopped"),
            Self::Destroyed => write!(f, "destroyed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Trusted peer spec (bridge-local, no meerkat-core dependency needed)
// ---------------------------------------------------------------------------

/// Minimal trusted peer identity for supervisor bridge wire messages.
///
/// Mirrors `meerkat_core::comms::TrustedPeerDescriptor` (post-C-TRP) but is
/// self-contained in the contracts crate so neither sender nor receiver
/// needs a cross-crate dependency for deserialization. Fields stay
/// stringly at the wire boundary — `peer_id` is the canonical comms routing
/// UUID, while raw Ed25519 public key material is carried only in `pubkey`.
/// The typed `PeerId`/`PeerName`/`PeerAddress` atoms are re-hydrated on the
/// receiving side.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgePeerSpec {
    pub name: String,
    pub peer_id: String,
    pub address: String,
    /// Ed25519 signing public key bytes — required so the receiver can
    /// verify envelope signatures after trust registration. Serialized
    /// as a 32-element array; the JSON form is an array of numbers, not
    /// a base64/hex string, to keep the wire deliberately boring.
    #[serde(default)]
    pub pubkey: [u8; 32],
}

/// Typed Ed25519 signing/trust subject carried by a bridge peer spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BridgePeerPubKey([u8; 32]);

impl BridgePeerPubKey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }

    pub const fn is_zero(&self) -> bool {
        let mut index = 0;
        while index < self.0.len() {
            if self.0[index] != 0 {
                return false;
            }
            index += 1;
        }
        true
    }

    pub fn derived_peer_id(&self) -> PeerId {
        PeerId::from_ed25519_pubkey(&self.0)
    }
}

impl From<[u8; 32]> for BridgePeerPubKey {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

/// Bridge peer identity after the stringly wire spec has crossed the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgePeerIdentity {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub pubkey: BridgePeerPubKey,
}

impl BridgePeerIdentity {
    pub fn into_trusted_peer_descriptor(self) -> TrustedPeerDescriptor {
        TrustedPeerDescriptor {
            peer_id: self.peer_id,
            name: self.name,
            address: self.address,
            pubkey: self.pubkey.into_bytes(),
        }
    }
}

/// Connectivity class observed for the bridged member runtime.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgePeerConnectivity {
    Reachable,
    Unreachable,
    Unknown,
}

impl From<TrustedPeerDescriptor> for BridgePeerSpec {
    fn from(spec: TrustedPeerDescriptor) -> Self {
        Self {
            name: spec.name.as_str().to_string(),
            peer_id: spec.peer_id.as_str(),
            address: spec.address.to_string(),
            pubkey: spec.pubkey,
        }
    }
}

impl TryFrom<BridgePeerSpec> for meerkat_core::comms::TrustedPeerDescriptor {
    type Error = String;

    fn try_from(spec: BridgePeerSpec) -> Result<Self, Self::Error> {
        Self::try_from(&spec)
    }
}

impl TryFrom<&BridgePeerSpec> for BridgePeerIdentity {
    type Error = String;

    fn try_from(spec: &BridgePeerSpec) -> Result<Self, Self::Error> {
        let peer_id = PeerId::parse(&spec.peer_id).map_err(|e| format!("invalid peer_id: {e}"))?;
        let name =
            PeerName::new(spec.name.clone()).map_err(|e| format!("invalid peer name: {e}"))?;
        let address = parse_peer_address(&spec.address)?;
        let pubkey = BridgePeerPubKey::new(spec.pubkey);
        if pubkey.is_zero() {
            return Err("peer pubkey must be non-zero".to_string());
        }
        let derived = pubkey.derived_peer_id();
        if derived != peer_id {
            return Err(format!(
                "peer_id {peer_id} does not match pubkey-derived id {derived}"
            ));
        }
        Ok(Self {
            name,
            peer_id,
            address,
            pubkey,
        })
    }
}

impl TryFrom<&BridgePeerSpec> for TrustedPeerDescriptor {
    type Error = String;

    fn try_from(spec: &BridgePeerSpec) -> Result<Self, Self::Error> {
        BridgePeerIdentity::try_from(spec).map(BridgePeerIdentity::into_trusted_peer_descriptor)
    }
}

fn parse_peer_address(raw: &str) -> Result<PeerAddress, String> {
    PeerAddress::parse(raw).map_err(|err| err.to_string())
}

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Supervisor authority credentials included in every bridge command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeSupervisorPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
}

/// Explicit hard-cancel command payload.
///
/// `InterruptMember` is the cooperative boundary-break path. This payload is
/// intentionally separate so supervisors cannot accidentally collapse boundary
/// cancellation and immediate user/session interrupt authority onto one wire
/// command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeHardCancelPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub reason: String,
}

/// One-time bootstrap proof exchanged between a mob supervisor and a
/// member runtime on initial bind.
///
/// Transparent over the wire (`#[serde(transparent)]` — a bare JSON string),
/// but carries a redacting `Debug` impl and has no `Display` impl so the
/// raw secret cannot accidentally land in logs or panic messages. Treat it
/// like an API key: read `as_str()` only at the comms/transport boundary.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct BridgeBootstrapToken(String);

impl BridgeBootstrapToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl From<String> for BridgeBootstrapToken {
    fn from(token: String) -> Self {
        Self(token)
    }
}

impl From<&str> for BridgeBootstrapToken {
    fn from(token: &str) -> Self {
        Self(token.to_string())
    }
}

impl std::fmt::Debug for BridgeBootstrapToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "BridgeBootstrapToken(empty)")
        } else {
            write!(f, "BridgeBootstrapToken(<redacted, {}B>)", self.0.len())
        }
    }
}

// Deliberately no `Display` impl: `Display` is the canonical "show me this
// value" path and the bootstrap token must never flow into a format string
// or user-facing surface. Call `as_str()` explicitly when bridging to the
// comms layer.

/// Bind a remote runtime to this supervisor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeBindPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Expected canonical member `PeerId`; not an Ed25519 public-key string.
    pub expected_peer_id: String,
    pub expected_address: String,
    pub bootstrap_token: BridgeBootstrapToken,
}

/// Capabilities advertised by a member runtime on bind.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeCapabilities {
    /// Protocol version implemented by the responding member runtime.
    #[serde(default = "supervisor_bridge_current_protocol_version")]
    pub current_protocol_version: BridgeProtocolVersion,
    /// Protocol version new supervisors should use for fresh authority records.
    #[serde(default = "supervisor_bridge_default_protocol_version")]
    pub default_protocol_version: BridgeProtocolVersion,
    /// Protocol versions accepted by the responding member runtime.
    #[serde(default = "default_supported_protocol_versions")]
    pub supported_protocol_versions: Vec<BridgeProtocolVersion>,
    #[serde(default)]
    pub deliver_member_input: bool,
    #[serde(default)]
    pub observe_member: bool,
    #[serde(default)]
    pub interrupt_member: bool,
    #[serde(default)]
    pub hard_cancel_member: bool,
    #[serde(default)]
    pub retire_member: bool,
    #[serde(default)]
    pub destroy_member: bool,
    #[serde(default)]
    pub wire_member: bool,
    #[serde(default)]
    pub unwire_member: bool,
}

impl Default for BridgeCapabilities {
    fn default() -> Self {
        Self {
            current_protocol_version: supervisor_bridge_current_protocol_version(),
            default_protocol_version: supervisor_bridge_default_protocol_version(),
            supported_protocol_versions: supervisor_bridge_supported_protocol_versions().to_vec(),
            deliver_member_input: false,
            observe_member: false,
            interrupt_member: false,
            hard_cancel_member: false,
            retire_member: false,
            destroy_member: false,
            wire_member: false,
            unwire_member: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Response to a bind command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeBindResponse {
    /// Canonical member `PeerId`; transport public-key bytes stay out of this field.
    pub peer_id: String,
    pub address: String,
    pub capabilities: BridgeCapabilities,
}

/// Simple acknowledgment.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeAck {
    pub ok: bool,
}

/// Deliver one logical input to a member.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub input_id: String,
    pub content: meerkat_core::types::ContentInput,
    pub handling_mode: meerkat_core::types::HandlingMode,
}

/// Outcome of a delivery attempt.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeDeliveryOutcome {
    Accepted,
    Deduplicated {
        existing_input_id: String,
    },
    Rejected {
        cause: BridgeDeliveryRejectionCause,
        reason: String,
    },
}

/// Typed vocabulary for why member input delivery was rejected.
///
/// This mirrors the runtime accept-boundary rejection vocabulary at the
/// bridge wire boundary. Callers should branch on this typed cause and treat
/// the sibling `reason` string on [`BridgeDeliveryOutcome::Rejected`] as
/// operator-facing presentation only.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeDeliveryRejectionCause {
    /// Runtime was not in a state that accepts input.
    NotReady { state: BridgeMemberRuntimeState },
    /// Input failed durability validation.
    DurabilityViolation { detail: String },
    /// Peer input carried a forbidden handling mode.
    PeerHandlingModeInvalid { detail: String },
    /// The bridge could not map the rejection to a known typed cause.
    Internal { detail: String },
}

impl std::fmt::Display for BridgeDeliveryRejectionCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady { state } => write!(f, "not_ready(state={state})"),
            Self::DurabilityViolation { detail } => {
                write!(f, "durability_violation(detail={detail})")
            }
            Self::PeerHandlingModeInvalid { detail } => {
                write!(f, "peer_handling_mode_invalid(detail={detail})")
            }
            Self::Internal { detail } => write!(f, "internal(detail={detail})"),
        }
    }
}

/// Full response to a delivery command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryResponse {
    pub input_id: String,
    pub canonical_input_id: Option<String>,
    pub outcome: BridgeDeliveryOutcome,
}

/// Peer wiring command payload.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgePeerWiringPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub peer_spec: BridgePeerSpec,
}

/// Response to a retire command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeRetireResponse {
    pub inputs_abandoned: usize,
    pub inputs_pending_drain: usize,
}

/// Response to a destroy command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeDestroyResponse {
    pub inputs_abandoned: usize,
}

/// Response to an observe command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeObservationResponse {
    /// Bridged runtime state for the observed member.
    pub state: BridgeMemberRuntimeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepting_inputs: Option<bool>,
    /// Current run identifier reported by the member runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_connectivity: Option<BridgePeerConnectivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// ISO 8601 timestamp.
    pub observed_at: String,
}

impl BridgeObservationResponse {
    /// Build an observation reply.
    pub fn new(
        state: BridgeMemberRuntimeState,
        accepting_inputs: Option<bool>,
        current_run_id: Option<String>,
        peer_connectivity: Option<BridgePeerConnectivity>,
        last_error: Option<String>,
        observed_at: String,
    ) -> Self {
        Self {
            state,
            accepting_inputs,
            current_run_id,
            peer_connectivity,
            last_error,
            observed_at,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn observation_response_new_sets_observation_fields() {
        let response = BridgeObservationResponse::new(
            BridgeMemberRuntimeState::Running,
            Some(true),
            Some("run-1".to_string()),
            Some(BridgePeerConnectivity::Reachable),
            None,
            "2026-04-16T07:00:00Z".to_string(),
        );

        assert_eq!(response.state, BridgeMemberRuntimeState::Running);
        assert_eq!(response.current_run_id.as_deref(), Some("run-1"));
        assert_eq!(response.accepting_inputs, Some(true));
        assert_eq!(
            response.peer_connectivity,
            Some(BridgePeerConnectivity::Reachable)
        );
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_peer_spec() -> BridgePeerSpec {
        BridgePeerSpec {
            name: "member-a".to_string(),
            peer_id: "peer-abc".to_string(),
            address: "tcp://127.0.0.1:7000".to_string(),
            pubkey: [0u8; 32],
        }
    }

    fn sample_supervisor_payload() -> BridgeSupervisorPayload {
        BridgeSupervisorPayload {
            supervisor: sample_peer_spec(),
            epoch: 42,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        }
    }

    fn sample_hard_cancel_payload() -> BridgeHardCancelPayload {
        BridgeHardCancelPayload {
            supervisor: sample_peer_spec(),
            epoch: 42,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            reason: "test hard cancel".to_string(),
        }
    }

    fn sample_wiring_payload() -> BridgePeerWiringPayload {
        BridgePeerWiringPayload {
            supervisor: sample_peer_spec(),
            epoch: 7,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            peer_spec: BridgePeerSpec {
                name: "member-b".to_string(),
                peer_id: "peer-xyz".to_string(),
                address: "tcp://127.0.0.1:7001".to_string(),
                pubkey: [0u8; 32],
            },
        }
    }

    // -----------------------------------------------------------------------
    // 1. BridgeCommand JSON round-trip — one subtest per variant.
    // -----------------------------------------------------------------------
    //
    // `BridgeCommand` does not derive `PartialEq`, so we assert round-trip
    // correctness by comparing `serde_json::Value` before and after a
    // decode/encode cycle. Equal JSON values imply semantic equality under
    // the wire contract.

    fn assert_command_round_trip(cmd: &BridgeCommand) {
        let value = serde_json::to_value(cmd).expect("serialize command");
        let decoded: BridgeCommand = serde_json::from_value(value.clone()).expect("decode command");
        let reencoded = serde_json::to_value(&decoded).expect("reserialize command");
        assert_eq!(
            value, reencoded,
            "BridgeCommand round-trip must preserve wire shape"
        );
    }

    #[test]
    fn bridge_command_unknown_top_level_field_fails_closed() {
        let mut value =
            serde_json::to_value(BridgeCommand::ObserveMember(sample_supervisor_payload()))
                .expect("serialize command");
        value["extra_behavior"] = json!(true);

        let err = serde_json::from_value::<BridgeCommand>(value)
            .expect_err("unknown command fields must fail at serde boundary");
        let message = err.to_string();
        assert!(
            message.contains("extra_behavior") || message.contains("unknown field"),
            "expected unknown field error, got: {message}"
        );
    }

    #[test]
    fn bridge_command_unknown_nested_payload_field_fails_closed() {
        let mut value =
            serde_json::to_value(BridgeCommand::ObserveMember(sample_supervisor_payload()))
                .expect("serialize command");
        value["supervisor"]["extra_behavior"] = json!(true);

        let err = serde_json::from_value::<BridgeCommand>(value)
            .expect_err("unknown nested payload fields must fail at serde boundary");
        let message = err.to_string();
        assert!(
            message.contains("extra_behavior") || message.contains("unknown field"),
            "expected unknown field error, got: {message}"
        );
    }

    #[test]
    fn bridge_command_bind_member_round_trip() {
        let cmd = BridgeCommand::BindMember(BridgeBindPayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "peer-expected".to_string(),
            expected_address: "tcp://127.0.0.1:9000".to_string(),
            bootstrap_token: "bootstrap-secret".into(),
        });
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_peer_spec_rejects_unknown_address_scheme() {
        let spec = BridgePeerSpec {
            name: "member-a".to_string(),
            peer_id: "aaaaaaaa-0000-4000-8000-000000000001".to_string(),
            address: "http://127.0.0.1:7000".to_string(),
            pubkey: [0u8; 32],
        };

        let err = meerkat_core::comms::TrustedPeerDescriptor::try_from(&spec)
            .expect_err("supervisor bridge peer specs must fail closed on unknown schemes");
        assert!(
            err.contains("unknown peer address transport"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn bridge_peer_spec_rejects_schemeless_address() {
        let spec = BridgePeerSpec {
            name: "member-a".to_string(),
            peer_id: "aaaaaaaa-0000-4000-8000-000000000001".to_string(),
            address: "127.0.0.1:7000".to_string(),
            pubkey: [0u8; 32],
        };

        let err = meerkat_core::comms::TrustedPeerDescriptor::try_from(&spec)
            .expect_err("supervisor bridge peer specs must fail closed on schemeless addresses");
        assert!(
            err.contains("missing transport scheme"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn bridge_peer_spec_rejects_zero_pubkey() {
        let spec = BridgePeerSpec {
            name: "member-a".to_string(),
            peer_id: PeerId::from_ed25519_pubkey(&[1u8; 32]).to_string(),
            address: "tcp://127.0.0.1:7000".to_string(),
            pubkey: [0u8; 32],
        };

        let err = meerkat_core::comms::TrustedPeerDescriptor::try_from(&spec)
            .expect_err("supervisor bridge peer specs must fail closed on zero pubkeys");
        assert!(
            err.contains("pubkey") && err.contains("non-zero"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn bridge_peer_spec_missing_pubkey_defaults_to_zero_and_rejects() {
        let value = json!({
            "name": "member-a",
            "peer_id": PeerId::from_ed25519_pubkey(&[2u8; 32]).to_string(),
            "address": "tcp://127.0.0.1:7000"
        });
        let spec: BridgePeerSpec =
            serde_json::from_value(value).expect("legacy bridge peer spec should deserialize");

        let err = meerkat_core::comms::TrustedPeerDescriptor::try_from(&spec)
            .expect_err("missing pubkey must not become trusted zero-key authority");
        assert!(
            err.contains("pubkey") && err.contains("non-zero"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn bridge_command_authorize_supervisor_round_trip() {
        let cmd = BridgeCommand::AuthorizeSupervisor(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_revoke_supervisor_round_trip() {
        let cmd = BridgeCommand::RevokeSupervisor(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_deliver_member_input_round_trip() {
        let cmd = BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: "input-1".to_string(),
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        });
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_observe_member_round_trip() {
        let cmd = BridgeCommand::ObserveMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_interrupt_member_round_trip() {
        let cmd = BridgeCommand::InterruptMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_hard_cancel_member_round_trip() {
        let cmd = BridgeCommand::HardCancelMember(sample_hard_cancel_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_retire_member_round_trip() {
        let cmd = BridgeCommand::RetireMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_destroy_member_round_trip() {
        let cmd = BridgeCommand::DestroyMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_wire_member_round_trip() {
        let cmd = BridgeCommand::WireMember(sample_wiring_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_unwire_member_round_trip() {
        let cmd = BridgeCommand::UnwireMember(sample_wiring_payload());
        assert_command_round_trip(&cmd);
    }

    // -----------------------------------------------------------------------
    // 2. BridgeReply::Rejected round-trip with typed cause.
    // -----------------------------------------------------------------------
    //
    // Pins the `{ result, cause, reason }` wire shape. Callers branch on
    // the typed `cause`; `reason` is a human-readable diagnostic and must
    // not be pattern-matched.

    #[test]
    fn bridge_reply_rejected_round_trip_with_typed_cause() {
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::StaleSupervisor,
            reason: "epoch too low".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejected reply");
        assert_eq!(
            value,
            json!({
                "result": "rejected",
                "cause": "stale_supervisor",
                "reason": "epoch too low",
            }),
            "wire shape must tag rejection with `result` + `cause` + `reason`"
        );
        let decoded: BridgeReply = serde_json::from_value(value.clone()).expect("decode reply");
        match decoded {
            BridgeReply::Rejected { cause, ref reason } => {
                assert_eq!(cause, BridgeRejectionCause::StaleSupervisor);
                assert_eq!(reason, "epoch too low");
            }
            other => panic!("expected BridgeReply::Rejected, got {other:?}"),
        }
        let reencoded = serde_json::to_value(&decoded).expect("reserialize reply");
        assert_eq!(value, reencoded);
    }

    #[test]
    fn bridge_rejection_decoder_accepts_typed_protocol_v2_rejection() {
        let value = json!({
            "result": "rejected",
            "cause": "sender_mismatch",
            "reason": "wrong supervisor",
        });

        let decoded = decode_bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value)
            .expect("typed rejection should decode");

        assert_eq!(
            decoded.typed_cause(),
            Some(BridgeRejectionCause::SenderMismatch)
        );
        assert_eq!(decoded.reason(), "wrong supervisor");
        assert!(!decoded.is_legacy_v1_raw_string());
    }

    #[test]
    fn bridge_rejection_decoder_rejects_raw_string_for_protocol_v2() {
        let value = json!("legacy rejection");

        assert!(
            decode_bridge_rejection_reply(SUPERVISOR_BRIDGE_PROTOCOL_VERSION, &value).is_none(),
            "protocol v2 must not promote raw strings into typed rejection causes"
        );
    }

    #[test]
    fn bridge_rejection_decoder_isolates_raw_string_to_legacy_v1() {
        let value = json!("legacy rejection");

        let decoded = decode_legacy_v1_raw_string_rejection(&value)
            .expect("legacy raw string should decode only through the explicit v1 helper");

        assert_eq!(decoded.typed_cause(), None);
        assert_eq!(decoded.reason(), "legacy rejection");
        assert!(decoded.is_legacy_v1_raw_string());
    }

    #[test]
    fn bridge_command_reports_payload_protocol_version() {
        let command = BridgeCommand::AuthorizeSupervisor(sample_supervisor_payload());

        assert_eq!(
            command.protocol_version(),
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
    }

    #[test]
    fn supervisor_bridge_protocol_versions_are_reported_from_single_authority() {
        assert_eq!(
            supervisor_bridge_current_protocol_version(),
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
        assert_eq!(
            supervisor_bridge_default_protocol_version(),
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
        assert_eq!(
            supervisor_bridge_supported_protocol_versions(),
            &[SUPERVISOR_BRIDGE_PROTOCOL_VERSION]
        );
        assert!(supervisor_bridge_protocol_version_supported(
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        ));
        assert!(BridgeProtocolVersion::from_supported_u32(1).is_err());
        assert!(BridgeProtocolVersion::from_supported_u32(999).is_err());
    }

    #[test]
    fn bridge_capabilities_default_reports_canonical_protocol_versions() {
        let capabilities = BridgeCapabilities::default();
        assert_eq!(
            capabilities.current_protocol_version,
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
        assert_eq!(
            capabilities.default_protocol_version,
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
        assert_eq!(
            capabilities.supported_protocol_versions,
            vec![SUPERVISOR_BRIDGE_PROTOCOL_VERSION]
        );
    }

    #[test]
    fn bridge_capabilities_deserialize_legacy_without_protocol_report() {
        let capabilities: BridgeCapabilities = serde_json::from_value(json!({
            "deliver_member_input": true,
            "observe_member": true,
            "interrupt_member": true,
            "retire_member": true,
            "destroy_member": true,
            "wire_member": true,
            "unwire_member": true,
        }))
        .expect("legacy capability payload without protocol report should decode");

        assert_eq!(
            capabilities.current_protocol_version,
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
        assert_eq!(
            capabilities.default_protocol_version,
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        );
        assert_eq!(
            capabilities.supported_protocol_versions,
            vec![SUPERVISOR_BRIDGE_PROTOCOL_VERSION]
        );
        assert!(capabilities.deliver_member_input);
        assert!(capabilities.observe_member);
        assert!(capabilities.interrupt_member);
        assert!(!capabilities.hard_cancel_member);
        assert!(capabilities.retire_member);
        assert!(capabilities.destroy_member);
        assert!(capabilities.wire_member);
        assert!(capabilities.unwire_member);
    }

    #[test]
    fn bridge_bind_payload_rejects_unsupported_protocol_version_at_wire_boundary() {
        let raw = json!({
            "supervisor": {
                "name": "mob/__mob_supervisor__",
                "peer_id": "00000000-0000-0000-0000-00000000bbbb",
                "address": "inproc://mob/__mob_supervisor__",
            },
            "epoch": 7,
            "protocol_version": 999,
            "expected_peer_id": "00000000-0000-0000-0000-00000000aaaa",
            "expected_address": "inproc://member",
            "bootstrap_token": "tok-raw-string",
        });

        let error = serde_json::from_value::<BridgeBindPayload>(raw)
            .expect_err("unsupported protocol versions must fail closed at decode");

        assert!(
            error
                .to_string()
                .contains("unsupported supervisor bridge protocol version"),
            "unexpected error: {error}",
        );
    }

    #[test]
    fn bridge_capabilities_reject_unsupported_protocol_versions_at_wire_boundary() {
        let raw = json!({
            "current_protocol_version": SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            "default_protocol_version": 999,
            "supported_protocol_versions": [SUPERVISOR_BRIDGE_PROTOCOL_VERSION],
            "deliver_member_input": true,
        });

        let error = serde_json::from_value::<BridgeCapabilities>(raw)
            .expect_err("unsupported advertised defaults must fail closed at decode");

        assert!(
            error
                .to_string()
                .contains("unsupported supervisor bridge protocol version"),
            "unexpected error: {error}",
        );
    }

    // -----------------------------------------------------------------------
    // 3. BridgeObservationResponse round-trip with all-present / all-absent
    //    optional fields. `skip_serializing_if = "Option::is_none"` must
    //    drop absent fields from the wire form.
    // -----------------------------------------------------------------------

    #[test]
    fn observation_response_round_trip_all_optional_present() {
        let response = BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Running,
            accepting_inputs: Some(true),
            current_run_id: Some("run-42".to_string()),
            peer_connectivity: Some(BridgePeerConnectivity::Reachable),
            last_error: Some("transient network blip".to_string()),
            observed_at: "2026-04-16T07:00:00Z".to_string(),
        };
        let value = serde_json::to_value(&response).expect("serialize observation");
        assert_eq!(
            value,
            json!({
                "state": "running",
                "accepting_inputs": true,
                "current_run_id": "run-42",
                "peer_connectivity": "reachable",
                "last_error": "transient network blip",
                "observed_at": "2026-04-16T07:00:00Z",
            })
        );
        let decoded: BridgeObservationResponse =
            serde_json::from_value(value.clone()).expect("decode observation");
        assert_eq!(decoded, response);
        let reencoded = serde_json::to_value(&decoded).expect("reserialize observation");
        assert_eq!(value, reencoded);
    }

    #[test]
    fn observation_response_round_trip_all_optional_absent() {
        let response = BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Idle,
            accepting_inputs: None,
            current_run_id: None,
            peer_connectivity: None,
            last_error: None,
            observed_at: "2026-04-16T07:01:00Z".to_string(),
        };
        let value = serde_json::to_value(&response).expect("serialize observation");
        assert_eq!(
            value,
            json!({
                "state": "idle",
                "observed_at": "2026-04-16T07:01:00Z",
            }),
            "absent optional fields must be skipped on the wire"
        );
        let decoded: BridgeObservationResponse =
            serde_json::from_value(value.clone()).expect("decode observation");
        assert_eq!(decoded, response);
        let reencoded = serde_json::to_value(&decoded).expect("reserialize observation");
        assert_eq!(value, reencoded);
    }

    // -----------------------------------------------------------------------
    // 4. BridgePeerConnectivity snake_case rename.
    // -----------------------------------------------------------------------

    #[test]
    fn peer_connectivity_serializes_as_snake_case() {
        for (variant, expected) in [
            (BridgePeerConnectivity::Reachable, "reachable"),
            (BridgePeerConnectivity::Unreachable, "unreachable"),
            (BridgePeerConnectivity::Unknown, "unknown"),
        ] {
            let value = serde_json::to_value(variant).expect("serialize connectivity");
            assert_eq!(
                value,
                json!(expected),
                "variant {variant:?} must serialize as {expected:?}"
            );
            let decoded: BridgePeerConnectivity =
                serde_json::from_value(value).expect("decode connectivity");
            assert_eq!(decoded, variant);
        }
    }

    // -----------------------------------------------------------------------
    // 5. BridgeMemberRuntimeState — every variant: Display + serde round-trip.
    // -----------------------------------------------------------------------

    #[test]
    fn member_runtime_state_display_and_round_trip_all_variants() {
        let cases: &[(BridgeMemberRuntimeState, &str)] = &[
            (BridgeMemberRuntimeState::Initializing, "initializing"),
            (BridgeMemberRuntimeState::Idle, "idle"),
            (BridgeMemberRuntimeState::Attached, "attached"),
            (BridgeMemberRuntimeState::Running, "running"),
            (BridgeMemberRuntimeState::Retired, "retired"),
            (BridgeMemberRuntimeState::Stopped, "stopped"),
            (BridgeMemberRuntimeState::Destroyed, "destroyed"),
        ];
        for (variant, expected) in cases {
            assert_eq!(
                variant.to_string(),
                *expected,
                "Display output must match snake_case wire form for {variant:?}"
            );
            let value = serde_json::to_value(variant).expect("serialize runtime state");
            assert_eq!(value, json!(expected));
            let decoded: BridgeMemberRuntimeState =
                serde_json::from_value(value).expect("decode runtime state");
            assert_eq!(decoded, *variant);
        }
    }

    // -----------------------------------------------------------------------
    // 6. BridgeRejectionCause — snake_case round-trip for every variant.
    // -----------------------------------------------------------------------
    //
    // Mob-side fallback logic (see `should_fall_back_to_bind`) branches on
    // typed causes. Any accidental rename or new variant that skipped the
    // snake_case convention would silently change fallback behavior, so pin
    // the full matrix.

    #[test]
    fn bridge_rejection_cause_snake_case_round_trip_all_variants() {
        let cases: &[(BridgeRejectionCause, &str)] = &[
            (BridgeRejectionCause::NotBound, "not_bound"),
            (BridgeRejectionCause::StaleSupervisor, "stale_supervisor"),
            (BridgeRejectionCause::SenderMismatch, "sender_mismatch"),
            (BridgeRejectionCause::AlreadyBound, "already_bound"),
            (
                BridgeRejectionCause::InvalidBootstrapToken,
                "invalid_bootstrap_token",
            ),
            (
                BridgeRejectionCause::UnsupportedProtocolVersion,
                "unsupported_protocol_version",
            ),
            (
                BridgeRejectionCause::InvalidSupervisorSpec,
                "invalid_supervisor_spec",
            ),
            (BridgeRejectionCause::InvalidPeerSpec, "invalid_peer_spec"),
            (BridgeRejectionCause::AddressMismatch, "address_mismatch"),
            (BridgeRejectionCause::Unsupported, "unsupported"),
            (BridgeRejectionCause::Internal, "internal"),
        ];
        for (cause, expected) in cases {
            let value = serde_json::to_value(cause).expect("serialize cause");
            assert_eq!(
                value,
                json!(expected),
                "cause {cause:?} must serialize as {expected:?}"
            );
            let decoded: BridgeRejectionCause =
                serde_json::from_value(value).expect("decode cause");
            assert_eq!(decoded, *cause);
        }
    }

    // -----------------------------------------------------------------------
    // 7. BridgeReply — protocol v2 round-trip for every success variant.
    // -----------------------------------------------------------------------
    //
    // The dispatcher in `comms_drain.rs` constructs exactly these variants;
    // the mob side decodes them via `send_bridge_command_typed`. A rename
    // would break the typed dispatch silently, so pin every `result` tag.

    fn assert_reply_round_trip(reply: BridgeReply, expected: serde_json::Value) {
        let value = serde_json::to_value(&reply).expect("serialize reply");
        assert_eq!(value, expected, "reply wire shape must be stable");
        let _decoded: BridgeReply = serde_json::from_value(value).expect("decode reply");
    }

    #[test]
    fn bridge_reply_unknown_field_fails_closed() {
        let err = serde_json::from_value::<BridgeReply>(json!({
            "result": "ack",
            "ok": true,
            "extra_behavior": true,
        }))
        .expect_err("unknown reply fields must fail at serde boundary");
        let message = err.to_string();
        assert!(
            message.contains("extra_behavior") || message.contains("unknown field"),
            "expected unknown field error, got: {message}"
        );
    }

    #[test]
    fn bridge_reply_bind_member_ack_round_trip() {
        assert_reply_round_trip(
            BridgeReply::BindMember(BridgeBindResponse {
                peer_id: "peer-x".to_string(),
                address: "inproc://peer-x".to_string(),
                capabilities: BridgeCapabilities::default(),
            }),
            json!({
                "result": "bind_member",
                "peer_id": "peer-x",
                "address": "inproc://peer-x",
                "capabilities": {
                    "current_protocol_version": SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                    "default_protocol_version": SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                    "supported_protocol_versions": [SUPERVISOR_BRIDGE_PROTOCOL_VERSION],
                    "deliver_member_input": false,
                    "observe_member": false,
                    "interrupt_member": false,
                    "hard_cancel_member": false,
                    "retire_member": false,
                    "destroy_member": false,
                    "wire_member": false,
                    "unwire_member": false,
                },
            }),
        );
    }

    #[test]
    fn bridge_reply_ack_round_trip() {
        assert_reply_round_trip(
            BridgeReply::Ack(BridgeAck { ok: true }),
            json!({ "result": "ack", "ok": true }),
        );
    }

    #[test]
    fn bridge_reply_observation_round_trip() {
        assert_reply_round_trip(
            BridgeReply::Observation(BridgeObservationResponse {
                state: BridgeMemberRuntimeState::Running,
                accepting_inputs: None,
                current_run_id: None,
                peer_connectivity: None,
                last_error: None,
                observed_at: "2026-04-17T00:00:00Z".to_string(),
            }),
            json!({
                "result": "observation",
                "state": "running",
                "observed_at": "2026-04-17T00:00:00Z",
            }),
        );
    }

    #[test]
    fn bridge_reply_delivery_round_trip() {
        assert_reply_round_trip(
            BridgeReply::Delivery(BridgeDeliveryResponse {
                input_id: "in-1".to_string(),
                canonical_input_id: None,
                outcome: BridgeDeliveryOutcome::Accepted,
            }),
            json!({
                "result": "delivery",
                "input_id": "in-1",
                "canonical_input_id": null,
                "outcome": { "outcome": "accepted" },
            }),
        );

        assert_reply_round_trip(
            BridgeReply::Delivery(BridgeDeliveryResponse {
                input_id: "in-2".to_string(),
                canonical_input_id: None,
                outcome: BridgeDeliveryOutcome::Rejected {
                    cause: BridgeDeliveryRejectionCause::DurabilityViolation {
                        detail: "derived durable input cannot be accepted".to_string(),
                    },
                    reason: "derived durable input cannot be accepted".to_string(),
                },
            }),
            json!({
                "result": "delivery",
                "input_id": "in-2",
                "canonical_input_id": null,
                "outcome": {
                    "outcome": "rejected",
                    "cause": {
                        "kind": "durability_violation",
                        "detail": "derived durable input cannot be accepted",
                    },
                    "reason": "derived durable input cannot be accepted",
                },
            }),
        );
    }

    #[test]
    fn bridge_delivery_not_ready_carries_typed_member_state() {
        let outcome = BridgeDeliveryOutcome::Rejected {
            cause: BridgeDeliveryRejectionCause::NotReady {
                state: BridgeMemberRuntimeState::Stopped,
            },
            reason: "runtime not accepting input while in state: stopped".to_string(),
        };
        let value = serde_json::to_value(&outcome).expect("serialize outcome");
        assert_eq!(
            value,
            json!({
                "outcome": "rejected",
                "cause": {
                    "kind": "not_ready",
                    "state": "stopped",
                },
                "reason": "runtime not accepting input while in state: stopped",
            })
        );

        let decoded: BridgeDeliveryOutcome =
            serde_json::from_value(value).expect("legacy wire state string remains compatible");
        assert_eq!(decoded, outcome);
    }

    #[test]
    fn bridge_reply_retire_round_trip() {
        assert_reply_round_trip(
            BridgeReply::Retire(BridgeRetireResponse {
                inputs_abandoned: 2,
                inputs_pending_drain: 0,
            }),
            json!({
                "result": "retire",
                "inputs_abandoned": 2,
                "inputs_pending_drain": 0,
            }),
        );
    }

    #[test]
    fn bridge_reply_destroy_round_trip() {
        assert_reply_round_trip(
            BridgeReply::Destroy(BridgeDestroyResponse {
                inputs_abandoned: 3,
            }),
            json!({
                "result": "destroy",
                "inputs_abandoned": 3,
            }),
        );
    }

    // -----------------------------------------------------------------------
    // 8. BridgeBootstrapToken::fmt::Debug must redact the body.
    // -----------------------------------------------------------------------
    //
    // The whole point of the newtype is that `{:?}` in tracing/panic
    // messages never leaks the raw secret. Regressing the Debug impl to the
    // default derive would silently reintroduce the leak, so pin the format.

    #[test]
    fn bridge_bootstrap_token_debug_redacts_nonempty_body() {
        let token = BridgeBootstrapToken::new("super-secret-bootstrap");
        let rendered = format!("{token:?}");
        assert_eq!(
            rendered,
            format!(
                "BridgeBootstrapToken(<redacted, {}B>)",
                "super-secret-bootstrap".len()
            )
        );
        assert!(
            !rendered.contains("super-secret-bootstrap"),
            "Debug output must not contain the raw token body"
        );
    }

    #[test]
    fn bridge_bootstrap_token_debug_marks_empty_token() {
        let token = BridgeBootstrapToken::new("");
        assert_eq!(format!("{token:?}"), "BridgeBootstrapToken(empty)");
    }

    // -----------------------------------------------------------------------
    // 9. BridgeBootstrapToken — `#[serde(transparent)]` round-trip.
    // -----------------------------------------------------------------------
    //
    // Wire format is a bare JSON string. Older clients and persisted
    // records that store `"tok-abc"` must still decode unchanged.

    #[test]
    fn bridge_bootstrap_token_serde_is_transparent_over_string() {
        let token = BridgeBootstrapToken::new("tok-abc");
        let value = serde_json::to_value(&token).expect("serialize token");
        assert_eq!(value, json!("tok-abc"));
        let decoded: BridgeBootstrapToken =
            serde_json::from_value(json!("tok-abc")).expect("decode token");
        assert_eq!(decoded, token);
        // And as a plain string
        let s = serde_json::to_string(&token).expect("serialize string");
        assert_eq!(s, "\"tok-abc\"");
    }

    // -----------------------------------------------------------------------
    // 12. Wire compat on M1: pre-newtype raw JSON must deserialize into the
    //     newtype BridgeBindPayload and serialize back byte-for-byte.
    // -----------------------------------------------------------------------
    //
    // Pins that introducing BridgeBootstrapToken did NOT break wire compat
    // with supervisors/members that emit a plain string bootstrap_token.

    #[test]
    fn bridge_bind_payload_wire_compat_with_plain_string_bootstrap_token() {
        let raw = json!({
            "supervisor": {
                "name": "mob/__mob_supervisor__",
                "peer_id": "00000000-0000-0000-0000-00000000bbbb",
                "address": "inproc://mob/__mob_supervisor__",
            },
            "epoch": 7,
            "protocol_version": SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            "expected_peer_id": "00000000-0000-0000-0000-00000000aaaa",
            "expected_address": "inproc://member",
            "bootstrap_token": "tok-raw-string",
        });
        let payload: BridgeBindPayload =
            serde_json::from_value(raw.clone()).expect("decode pre-newtype payload");
        assert_eq!(payload.bootstrap_token.as_str(), "tok-raw-string");
        assert_eq!(payload.supervisor.pubkey, [0u8; 32]);
        let reencoded = serde_json::to_value(&payload).expect("reserialize payload");
        let mut expected = raw;
        expected["supervisor"]["pubkey"] = json!(vec![0u8; 32]);
        assert_eq!(
            reencoded, expected,
            "pre-pubkey payloads must decode and reserialize with the defaulted pubkey"
        );
    }
}
