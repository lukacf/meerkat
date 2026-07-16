//! Supervisor bridge protocol wire types.
//!
//! Typed wire envelope for cross-machine bridge commands between a mob
//! supervisor and the runtime instances it manages. Both `meerkat-mob`
//! (sender) and `meerkat-runtime` (receiver) consume these types. Neither
//! crate depends on the other — the contracts crate owns the vocabulary.

use super::connection::WireAuthBindingRef;
use super::live::{
    LiveCloseStatus, LiveCommitInputStatus, LiveInterruptStatus, LiveOpenResult, LiveOpenTransport,
    LiveRefreshStatus, LiveTruncateStatus, WireLiveAdapterStatus,
};
use super::mob::{
    WireControlScope, WireMemberHistoryPageBody, WireMemberLaunchMode, WireMobRuntimeMode,
    WireTrustedPeerIdentity,
};
use super::portable_spec::{PortableMemberSpec, WireResolvedToolAccessPolicy};
use super::realtime::RealtimeTurningMode;
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
    command: Option<&'static str>,
    minimum: Option<u32>,
}

impl fmt::Display for UnsupportedBridgeProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.command, self.minimum) {
            (Some(command), Some(minimum)) => write!(
                f,
                "unsupported supervisor bridge protocol version {} for {command} (minimum {minimum}; supported {:?}; default {})",
                self.raw,
                BridgeProtocolVersion::SUPPORTED,
                BridgeProtocolVersion::DEFAULT
            ),
            _ => write!(
                f,
                "unsupported supervisor bridge protocol version {} (supported {:?}; default {})",
                self.raw,
                BridgeProtocolVersion::SUPPORTED,
                BridgeProtocolVersion::DEFAULT
            ),
        }
    }
}

impl std::error::Error for UnsupportedBridgeProtocolVersion {}

impl BridgeProtocolVersion {
    /// Protocol with typed rejection causes.
    pub const V2: Self = Self(2);
    /// Protocol carrying the MobMachine peer overlay on peer wiring commands.
    pub const V3: Self = Self(3);
    /// Multi-host protocol: host-addressed family (`BindHost`, `RebindHost`,
    /// `RevokeHost`,
    /// `MaterializeMember`, `ReleaseMember`, peer-trust installs,
    /// `HostStatus`), observation family (`ReadMemberHistory`,
    /// `PollMemberEvents`), live-channel family, the member-originated
    /// `MemberOperatorRequest` family, exact-run `HardCancelMember`, and the
    /// optional `DeliverMemberInput.turn` / `expected_member` /
    /// `outcome_tracking` / `transcript_interaction_id` extensions.
    pub const V4: Self = Self(4);
    /// Current protocol version implemented by this bridge contract.
    pub const CURRENT: Self = Self::V4;
    /// Default protocol version for newly minted supervisor authority.
    ///
    /// V4 is a semantic fold: it carries both the multi-host families and the
    /// durable supervisor-rotation protocol. V2/V3 remain decodable for
    /// persisted or negotiated legacy peers, but new authorities must not
    /// silently omit V4 rotation semantics.
    pub const DEFAULT: Self = Self::V4;
    /// Protocol versions accepted by this bridge contract.
    ///
    /// V2 is retained so persisted supervisor-authority records and V2 peers
    /// keep loading and decoding. Peer wiring (`WireMember`/`UnwireMember`),
    /// which requires the MobMachine peer overlay, demands V3; the
    /// multi-host command families demand V4. In both cases an
    /// under-versioned command is rejected with a typed
    /// `UnsupportedProtocolVersion` cause rather than a raw
    /// deserialization error.
    pub const SUPPORTED: &'static [Self] = &[Self::V2, Self::V3, Self::V4];

    pub const fn is_supported(self) -> bool {
        matches!(self.0, 2..=4)
    }

    pub const fn same_protocol_as(self, other: Self) -> bool {
        self.0 == other.0
    }

    /// Whether this protocol version carries the MobMachine peer overlay on
    /// peer wiring commands. V2 predates the overlay; V3 and later require it.
    pub const fn supports_peer_overlay(self) -> bool {
        self.0 >= 3
    }

    /// Whether this protocol version carries the multi-host command families
    /// (host-addressed, observation, live-channel, member-originated) and
    /// the `DeliverMemberInput.turn` directive and explicit interaction
    /// outcome tracking. V4 and later.
    pub const fn supports_multi_host(self) -> bool {
        self.0 >= 4
    }

    pub fn supported() -> &'static [Self] {
        Self::SUPPORTED
    }

    fn from_supported_u32(raw: u32) -> Result<Self, UnsupportedBridgeProtocolVersion> {
        match raw {
            2 => Ok(Self::V2),
            3 => Ok(Self::V3),
            4 => Ok(Self::V4),
            _ => Err(UnsupportedBridgeProtocolVersion {
                raw,
                command: None,
                minimum: None,
            }),
        }
    }

    fn insufficient_for_command(
        self,
        command: &'static str,
        minimum: Self,
    ) -> UnsupportedBridgeProtocolVersion {
        UnsupportedBridgeProtocolVersion {
            raw: self.0,
            command: Some(command),
            minimum: Some(minimum.0),
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
/// - `3`: `WireMember`/`UnwireMember` carry the MobMachine peer overlay
///   (`BridgePeerWiringPayload::mob_peer_overlay`), which the receiver folds
///   into the generated direct-peer-endpoint projection. The field is
///   `#[serde(default, skip_serializing_if)]` so a V2 wiring payload (no
///   overlay) still deserializes on a V3 receiver; the receiver then rejects
///   it with a typed `UnsupportedProtocolVersion` cause instead of a raw
///   serde error. V3 supervisor authorities defaulted to this version.
/// - `3` (additive, no bump): `DeliverMemberInput` payloads may carry
///   `BridgeDeliveryPayload::injected_context`. Empty is omitted, so
///   deliveries without injected context stay byte-identical in both
///   directions. A non-empty value sent to a pre-field receiver fails loud
///   at its `deny_unknown_fields` serde boundary (pre-1.0 posture). Unlike
///   the V3 overlay, no receiver-side semantic fold depends on the field
///   being understood to keep the rest of the payload sound, so the
///   version-gated typed rejection the V3 bump bought is not demanded here.
/// - `3` (additive, no bump): `DeclareMemberOutboundTaint` relays the host's
///   outbound content-taint declaration to a remote member runtime. A new
///   command VARIANT (not a field): a pre-variant receiver fails loud at the
///   tagged-enum serde boundary and the supervisor sees a typed decode
///   rejection — no silent drop, and no receiver-side semantic fold depends
///   on partial understanding, so the V3-overlay-style version bump is not
///   demanded (pre-1.0 posture, same reasoning as `injected_context`).
/// - `4`: supervisor rotation becomes an operation-correlated durable
///   protocol, and multi-host mobs add the host-addressed family (`BindHost`,
///   `RebindHost`, `RevokeHost`,
///   `MaterializeMember`, `ReleaseMember`, `InstallPeerTrust` /
///   `RemovePeerTrust`, `HostStatus`), member observation family
///   (`ReadMemberHistory`, `PollMemberEvents`), member live-channel family
///   (`OpenMemberLiveChannel` / `CloseMemberLiveChannel` /
///   `MemberLiveChannelStatus` / `ControlMemberLiveChannel`), exactly one
///   member-originated family (`MemberOperatorRequest`), exact-run
///   `HardCancelMember`, and the optional `DeliverMemberInput.turn` /
///   `expected_member` / `transcript_interaction_id` extensions
///   (absent-omitted; an extended delivery fails closed on pre-V4 receivers
///   via `deny_unknown_fields`).
///   The new supervisor authority is submitted only through the closed
///   one-way [`BridgeSupervisorDelivery`] envelope, while
///   [`BridgeCommand::ObserveSupervisorRotation`] observes the pending or
///   terminal receipt.
///   New-command payloads stamp V4; V2/V3 remain explicitly negotiable for
///   their pre-V4 command vocabulary.
/// - `4` (additive, no bump): `DeliverMemberInput` carries the optional
///   delegated objective identity. `None` is omitted for byte compatibility;
///   a present value must fail loud on an older strict receiver rather than
///   silently losing objective causality.
/// - `4` (additive, no bump): plain member-addressed `DeliverMemberInput` may
///   explicitly request durable terminal custody with
///   `outcome_tracking: "interaction"`. The marker is absent-omitted, so all
///   pre-field deliveries keep their exact wire shape and remain untracked
///   unless they carry the existing V4 `turn` directive. The marker is mutually
///   exclusive with `turn`: flow-step work already owns directed terminal
///   custody through its directive.
/// - `4` (additive, no bump): placed plain delivery may carry an independent
///   `transcript_interaction_id`. Admission keeps the transport/idempotency
///   `input_id` separate from caller transcript identity; tracked completion
///   uses the same canonical UUID for both so its retained sidecar remains
///   exactly correlated.
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

fn bool_is_false(value: &bool) -> bool {
    !*value
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

/// Closed one-way supervisor delivery envelope.
///
/// This marker is deliberately distinct from [`BridgeCommand`]. A member may
/// decode a peer-message body as this type to admit supervisor-owned one-way
/// control traffic without interpreting arbitrary user message JSON as a
/// request/reply bridge command. Supervisor rotation submission is the only
/// admitted one-way delivery.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "delivery", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeSupervisorDelivery {
    SubmitSupervisorRotation(BridgeSupervisorRotationSubmit),
}

impl BridgeSupervisorDelivery {
    /// Protocol version carried by this one-way delivery.
    pub fn protocol_version(&self) -> BridgeProtocolVersion {
        match self {
            Self::SubmitSupervisorRotation(payload) => payload.protocol_version,
        }
    }
}

/// A typed command sent from a supervisor to a member runtime, a mob host
/// daemon (host-addressed family), or — for exactly one family
/// (`MemberOperatorRequest`) — from a member host to the controlling host.
///
/// The full `PartialEq + Eq` chain is load-bearing: the comms envelope
/// enums (`CommsPeerRequestParams` et al.) derive `Eq` over this type.
/// Opaque JSON in payloads therefore rides [`WireOpaqueJson`], never a
/// bare `serde_json::Value`.
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
    InterruptMember(BridgeInterruptPayload),
    HardCancelMember(BridgeHardCancelPayload),
    /// Level-triggered cancellation of one exact tracked delivery. Unlike
    /// `HardCancelMember`, this command is fenced by the durable delivery
    /// idempotency key rather than a transient run id, so it can close the
    /// stop-before-first-send window and survive host restart.
    CancelTrackedMemberInput(BridgeTrackedInputCancelPayload),
    RetireMember(BridgeSupervisorPayload),
    DestroyMember(BridgeSupervisorPayload),
    WireMember(BridgePeerWiringPayload),
    UnwireMember(BridgePeerWiringPayload),
    DeclareMemberOutboundTaint(BridgeOutboundTaintPayload),
    // --- V4 member-addressed observation family ---
    ReadMemberHistory(BridgeReadHistoryPayload),
    PollMemberEvents(BridgePollEventsPayload),
    // --- V4 member-addressed live-channel family ---
    OpenMemberLiveChannel(BridgeLiveOpenPayload),
    CloseMemberLiveChannel(BridgeLiveChannelPayload),
    MemberLiveChannelStatus(BridgeLiveStatusPayload),
    ControlMemberLiveChannel(BridgeLiveControlPayload),
    // --- V4 host-addressed family (epoch = HOST authority epoch) ---
    BindHost(BridgeHostBindPayload),
    RebindHost(BridgeHostRebindPayload),
    RevokeHost(BridgeHostRevokePayload),
    /// Boxed: the portable spec dwarfs every sibling payload.
    MaterializeMember(Box<BridgeMaterializePayload>),
    ReleaseMember(BridgeReleasePayload),
    InstallPeerTrust(BridgePeerTrustPayload),
    RemovePeerTrust(BridgePeerTrustPayload),
    HostStatus(BridgeHostStatusPayload),
    // --- V4 member-originated family (exactly one, A8) ---
    MemberOperatorRequest(BridgeMemberOperatorPayload),
    /// Observe one previously submitted durable supervisor rotation.
    ObserveSupervisorRotation(BridgeSupervisorRotationObserve),
}

impl BridgeCommand {
    /// Protocol version carried by this command's payload.
    pub fn protocol_version(&self) -> BridgeProtocolVersion {
        match self {
            Self::BindMember(payload) => payload.protocol_version,
            Self::AuthorizeSupervisor(payload)
            | Self::RevokeSupervisor(payload)
            | Self::ObserveMember(payload)
            | Self::RetireMember(payload)
            | Self::DestroyMember(payload) => payload.protocol_version,
            Self::InterruptMember(payload) => payload.protocol_version,
            Self::HardCancelMember(payload) => payload.protocol_version,
            Self::CancelTrackedMemberInput(payload) => payload.protocol_version,
            Self::DeliverMemberInput(payload) => payload.protocol_version,
            Self::WireMember(payload) | Self::UnwireMember(payload) => payload.protocol_version,
            Self::DeclareMemberOutboundTaint(payload) => payload.protocol_version,
            Self::ReadMemberHistory(payload) => payload.protocol_version,
            Self::PollMemberEvents(payload) => payload.protocol_version,
            Self::OpenMemberLiveChannel(payload) => payload.protocol_version,
            Self::CloseMemberLiveChannel(payload) => payload.protocol_version,
            Self::MemberLiveChannelStatus(payload) => payload.protocol_version,
            Self::ControlMemberLiveChannel(payload) => payload.protocol_version,
            Self::BindHost(payload) => payload.protocol_version,
            Self::RebindHost(payload) => payload.protocol_version,
            Self::RevokeHost(payload) => payload.protocol_version,
            Self::MaterializeMember(payload) => payload.protocol_version,
            Self::ReleaseMember(payload) => payload.protocol_version,
            Self::InstallPeerTrust(payload) | Self::RemovePeerTrust(payload) => {
                payload.protocol_version
            }
            Self::HostStatus(payload) => payload.protocol_version,
            Self::MemberOperatorRequest(payload) => payload.protocol_version,
            Self::ObserveSupervisorRotation(payload) => payload.protocol_version,
        }
    }
}

/// Declare (or clear) the member's host-owned outbound content-taint
/// declaration.
///
/// The supervisor relays the HOST's declaration — the host owns the
/// "this member's session content is tainted" fact; the member runtime is
/// the authenticated carrier, stamping the declaration inside the signed
/// region of every outbound content-bearing envelope until changed.
/// `taint: None` clears the declaration (subsequent envelopes carry no
/// claim, which receivers must never coalesce into `Clean`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeOutboundTaintPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Routing authority introduced in V4. Legacy V2/V3 payloads omit this
    /// field and are admitted only by a peer-only receiving session. A V4
    /// `Placed` target is matched byte-for-byte against the registered
    /// incarnation before mutation; `PeerOnly` is accepted only by a session
    /// with no host incarnation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<BridgeOutboundTaintTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taint: Option<meerkat_core::comms::SenderContentTaint>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeOutboundTaintTarget {
    PeerOnly,
    Placed(BridgeMemberIncarnation),
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
    let version = value
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
        .map(BridgeProtocolVersion::from_supported_u32)
        .transpose()
        .map_err(BridgeCommandDecodeError::UnsupportedProtocolVersion)?;
    if let Some((command, minimum)) = bridge_command_minimum_protocol(&value)
        && let Some(version) = version
        && version < minimum
    {
        return Err(BridgeCommandDecodeError::UnsupportedProtocolVersion(
            version.insufficient_for_command(command, minimum),
        ));
    }
    serde_json::from_value(value).map_err(BridgeCommandDecodeError::Invalid)
}

/// Minimum wire version for every command family whose shape or semantics was
/// introduced after V2. This check intentionally runs on the tagged JSON
/// envelope before payload deserialization, so an old V3 HardCancel shape is
/// rejected as a typed version mismatch rather than an opaque missing-field
/// serde error. V2 WireMember/UnwireMember are deliberately absent: their
/// overlay field is backward-decodable and the serving boundary owns the
/// established typed rejection for a missing V3 overlay.
fn bridge_command_minimum_protocol(
    value: &serde_json::Value,
) -> Option<(&'static str, BridgeProtocolVersion)> {
    let command = value.get("command")?.as_str()?;
    let minimum = match command {
        "hard_cancel_member"
        | "cancel_tracked_member_input"
        | "read_member_history"
        | "poll_member_events"
        | "open_member_live_channel"
        | "close_member_live_channel"
        | "member_live_channel_status"
        | "control_member_live_channel"
        | "bind_host"
        | "rebind_host"
        | "revoke_host"
        | "materialize_member"
        | "release_member"
        | "install_peer_trust"
        | "remove_peer_trust"
        | "host_status"
        | "member_operator_request"
        | "observe_supervisor_rotation" => BridgeProtocolVersion::V4,
        "deliver_member_input"
            if value
                .get("outcome_tracking")
                .is_some_and(|tracking| !tracking.is_null())
                || value
                    .get("transcript_interaction_id")
                    .is_some_and(|interaction_id| !interaction_id.is_null()) =>
        {
            BridgeProtocolVersion::V4
        }
        "deliver_member_input"
            if value.get("turn").is_some_and(|turn| !turn.is_null())
                || value
                    .get("expected_member")
                    .is_some_and(|expected| !expected.is_null()) =>
        {
            BridgeProtocolVersion::V4
        }
        "interrupt_member"
            if value
                .get("expected_member")
                .is_some_and(|expected| !expected.is_null()) =>
        {
            BridgeProtocolVersion::V4
        }
        "declare_member_outbound_taint" if value.get("target").is_some() => {
            BridgeProtocolVersion::V4
        }
        _ => return None,
    };
    let command = match command {
        "hard_cancel_member" => "HardCancelMember",
        "cancel_tracked_member_input" => "CancelTrackedMemberInput",
        "read_member_history" => "ReadMemberHistory",
        "poll_member_events" => "PollMemberEvents",
        "open_member_live_channel" => "OpenMemberLiveChannel",
        "close_member_live_channel" => "CloseMemberLiveChannel",
        "member_live_channel_status" => "MemberLiveChannelStatus",
        "control_member_live_channel" => "ControlMemberLiveChannel",
        "bind_host" => "BindHost",
        "rebind_host" => "RebindHost",
        "revoke_host" => "RevokeHost",
        "materialize_member" => "MaterializeMember",
        "release_member" => "ReleaseMember",
        "install_peer_trust" => "InstallPeerTrust",
        "remove_peer_trust" => "RemovePeerTrust",
        "host_status" => "HostStatus",
        "member_operator_request" => "MemberOperatorRequest",
        "observe_supervisor_rotation" => "ObserveSupervisorRotation",
        "deliver_member_input" => "DeliverMemberInput(V4 extension)",
        "interrupt_member" => "InterruptMember(V4 extension)",
        "declare_member_outbound_taint" => "DeclareMemberOutboundTaint(V4 target)",
        _ => unreachable!("minimum-version command table is exhaustive"),
    };
    Some((command, minimum))
}

// ---------------------------------------------------------------------------
// V4 payloads — member-addressed observation + live families
// ---------------------------------------------------------------------------

/// Read a transcript snapshot page from a remote member (V4).
///
/// Serving semantics (DEC-P6E-6): `from_index: Some(i)` serves from offset
/// `i`; `from_index: None` with `limit: Some(n)` is TAIL-ADDRESSED — the
/// server computes `offset = message_count - n` and serves the last `n`
/// messages (the page carries the real `from_index` + `message_count`, so
/// client offset math is exact in one round trip); `from_index: None,
/// limit: None` serves the full transcript from 0, server-capped per page
/// with `next_index`/`complete` driving continuation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeReadHistoryPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Long-poll a remote member's durable event stream (V4).
///
/// `wait_ms` must stay bounded well under the bridge transport budget so a
/// long-poll never masquerades as a dead peer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePollEventsPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    pub cursor: BridgeEventCursor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    /// Exact terminal rows successfully consumed from the previous page.
    /// The member host prunes only matching durable rows; an acknowledgement
    /// for a row that has not committed yet is a no-op and creates no
    /// tombstone, so a delayed journal commit cannot be skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcome_acks: Vec<BridgeTurnOutcomeAck>,
    /// Maximum unacknowledged terminal rows to return on this page. The host
    /// applies its own smaller ceiling regardless of the requested value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_outcomes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u32>,
}

/// Cursor into a member's event stream.
///
/// `seq` is the owning host's durable `StoredEvent.seq` for the session
/// bound at `generation` — NEVER a session-task or per-stream counter. A
/// fresh-session respawn starts a new seq domain, while same-session Resume
/// continues that session's seqs behind a host-recorded generation floor;
/// page replies always carry the current generation so consumers restart at
/// the owning host's resolved floor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cursor", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeEventCursor {
    /// Start at the live tail (skip history).
    Tail,
    /// Resume from a recorded `(generation, seq)` position.
    At { generation: u64, seq: u64 },
}

/// Resolved `(generation, seq)` event position for DTO-side projections.
///
/// Same seq domain as [`BridgeEventCursor`]: durable `StoredEvent.seq` only.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberEventCursor {
    pub generation: u64,
    pub seq: u64,
}

/// Open a live channel on a remote member (V4). Mirrors `LiveOpenParams`
/// minus `session_id` — identity addressing replaces it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLiveOpenPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turning_mode: Option<RealtimeTurningMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<LiveOpenTransport>,
}

/// Address one live channel on a remote member (close, V4). The id is
/// REQUIRED: close-what-you-name is the CAS-like property that keeps a
/// reconciling console from race-killing a channel a concurrent legitimate
/// open just minted (§16.6 / ADJ-P6B-2).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLiveChannelPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    pub channel_id: String,
}

/// Point-read one live channel on a remote member (status, V4; ADJ-P6B-2).
///
/// `channel_id: None` addresses "the member's active channel": the member
/// resolves `live_active_channel_by_session` for its bound session and the
/// reply echoes the resolved id; no active channel is a typed
/// `LiveChannelNotFound` rejection. This optional addressing is what makes
/// a reply-lost open's orphaned binding DISCOVERABLE (§16.6) without any
/// controlling-side channel map.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLiveStatusPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
}

/// Drive one live-channel control verb on a remote member (V4).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLiveControlPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    pub channel_id: String,
    pub verb: BridgeLiveControlVerb,
}

/// Live-channel control verbs. `Truncate` mirrors `LiveTruncateParams`
/// minus `channel_id` (carried by the payload).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeLiveControlVerb {
    CommitInput,
    Interrupt,
    Truncate {
        item_id: String,
        content_index: u32,
        audio_played_ms: u64,
    },
    Refresh,
}

// ---------------------------------------------------------------------------
// V4 payloads — host-addressed family
// ---------------------------------------------------------------------------

/// Capabilities the controller requires the host to preserve before a bind
/// or rebind may become durable. The clause is computed from existing placed
/// residency/custody and validated host-side before authority mutation, so an
/// accepted remote ceremony can never be rejected only afterward by the
/// controller's machine.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostCapabilityRequirements {
    pub durable_sessions: bool,
    pub autonomous_members: bool,
    pub tracked_input_cancel: bool,
    pub protocol_v4: bool,
}

/// Dedicated host-bind proof carrier. This is intentionally distinct from
/// [`BridgeBootstrapToken`]: the raw bearer lives only in the out-of-band
/// host descriptor, while a `BindHost` command may carry only the
/// request-bound HMAC derived from it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct BridgeHostBootstrapProof(String);

impl BridgeHostBootstrapProof {
    pub fn new(proof: impl Into<String>) -> Self {
        Self(proof.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl std::fmt::Debug for BridgeHostBootstrapProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            write!(f, "BridgeHostBootstrapProof(empty)")
        } else {
            write!(f, "BridgeHostBootstrapProof(<redacted, {}B>)", self.0.len())
        }
    }
}

/// Bind a mob host daemon to this supervisor (V4). `epoch` is the HOST
/// authority epoch, a separate domain from member supervisor epochs.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostBindPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    /// Durable per-host binding generation. Fresh binds strictly advance;
    /// every host-addressed command is admitted only at the exact active
    /// generation so delayed work cannot cross revoke + replacement bind.
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Mob the host is being bound for. Explicit and required: the host
    /// authority is mob-keyed, and deriving mob identity from
    /// `supervisor.name` is forbidden (display names are never identity).
    pub mob_id: String,
    /// Expected canonical host `PeerId`; not an Ed25519 public-key string.
    pub expected_host_peer_id: String,
    pub expected_address: String,
    /// Domain-separated HMAC proof derived from the raw one-time token in the
    /// out-of-band host descriptor. The raw bearer token must never be sent
    /// in this signed-but-unencrypted bridge command.
    pub bootstrap_proof: BridgeHostBootstrapProof,
    pub required_capabilities: BridgeHostCapabilityRequirements,
}

/// Re-bind an already-bound mob host daemon under a rotated supervisor
/// authority (V4, §7.2 step 4). The `supervisor` field declares the NEXT
/// authority, while an advancing request MUST be envelope-signed by the
/// host's recorded CURRENT authority. A request signed by the next authority
/// is accepted only as an exact replay after that tuple was committed. This
/// old-authority proof prevents an untrusted peer from self-authorizing a
/// higher epoch. The reply re-declares capabilities (restart-truthfulness,
/// §14.5): rebind capability facts REPLACE the bind-time record, and an
/// absent `live_endpoint` clears the recorded one.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostRebindPayload {
    /// Proposed next supervisor. This is not proof of authority: the signed
    /// envelope's authenticated sender is checked against the recorded
    /// current supervisor before an epoch advance is admitted.
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Mob whose host binding is being rotated (mob-keyed authority; never
    /// derived from a display name).
    pub mob_id: String,
    pub required_capabilities: BridgeHostCapabilityRequirements,
}

/// Revoke a mob's binding from its owning host daemon (V4).
///
/// This is an authenticated host-addressed terminal, not a controlling-side
/// bookkeeping shortcut. While the binding is live, the envelope signer and
/// `epoch` must exactly match the host's durable current supervisor tuple.
/// The host disposes every materialized runtime and atomically replaces the
/// active binding row with a durable revoke receipt before replying. Exact
/// redelivery at the same tuple replays [`BridgeHostRevokedResponse`]; a
/// delayed old revoke can never cross a fresh replacement bind.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostRevokePayload {
    /// Current supervisor transport descriptor. It supplies the declared
    /// reply address only; authority comes from the authenticated envelope
    /// signer matched against durable host truth.
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub mob_id: String,
}

/// Materialize (build + run) a member on the bound host (V4).
///
/// Idempotent on `(agent_identity, generation, fence_token)`: replay
/// returns the recorded result; a lower tuple is a typed `StaleFence`
/// reject; the same tuple with a different `spec_digest` is
/// `SpecDigestMismatch` (A12 — one idempotency key never names two builds).
/// `launch` is the ONLY launch-mode carrier (A6): the spec has no
/// launch-mode field. Budget has exactly ONE wire carrier (§18 O4): the
/// digest-covered `spec.overlay.budget_limits` — the member host applies
/// it at session build, and revival recomposes it from the durable spec
/// row; there is deliberately no payload-level budget sibling. Member
/// identity likewise has exactly one carrier: digest-covered
/// `spec.agent_identity`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMaterializePayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub generation: u64,
    pub fence_token: u64,
    pub spec: PortableMemberSpec,
    /// Canonical digest of `spec` (`portable_member_spec_digest`).
    pub spec_digest: String,
    pub launch: MaterializeLaunchMode,
}

/// Launch mode for `MaterializeMember`. CLOSED — `Fork` is deliberately
/// unrepresentable on this wire (A6/§19.L1): mob fork collapses to prompt
/// text on the controlling host before provisioning, and resume validation
/// is realm-local to the member host. A `{"mode":"fork"}` payload is a
/// fail-closed decode error, never a fallback.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterializeLaunchMode {
    // Empty struct variant, not unit: serde's `deny_unknown_fields` does not
    // apply to unit variants of internally-tagged enums, so a unit `Fresh`
    // would silently accept smuggled sibling fields. Wire bytes are identical
    // ({"mode":"fresh"}).
    Fresh {},
    Resume { session_id: String },
}

/// Which launch path actually ran on the member host (§19.L1).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializeLaunchOutcome {
    Fresh,
    ResumedLive,
    ResumedFromSnapshot,
}

/// Fully release a materialized member on its owning host (V4, §19.L3).
/// Same idempotency tuple discipline as `MaterializeMember`; doubles as the
/// orphan-reconciliation verb.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeReleasePayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Mob scope for the release (host authority facts are mob-keyed; never
    /// derived from a display name).
    pub mob_id: String,
    pub agent_identity: String,
    pub generation: u64,
    pub fence_token: u64,
}

/// Install or remove one trusted peer on a materialized member, through the
/// member's machine-gated trust seam — never direct TrustStore writes (V4).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePeerTrustPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Mob scope for the trust mutation (host authority facts are mob-keyed;
    /// never derived from a display name).
    pub mob_id: String,
    pub agent_identity: String,
    pub peer: BridgePeerSpec,
}

/// Query the host's materialized-member inventory + health (V4). Feeds
/// reconciliation and the reachability projection.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostStatusPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub binding_generation: u64,
    pub protocol_version: BridgeProtocolVersion,
    /// Mob whose materialized inventory is being queried (host authority
    /// facts are mob-keyed; never derived from a display name).
    pub mob_id: String,
}

// ---------------------------------------------------------------------------
// V4 payloads — member-originated operator family (exactly one, A8)
// ---------------------------------------------------------------------------

/// Member-originated operator request forwarded from the member's host to
/// the controlling host (V4). Carries NO authority claim — admission is
/// MobMachine-owned (`ResolveMemberOperatorAdmission`) and the recorded
/// authority context is read controlling-host-side.
///
/// `request_id` is the idempotency key: duplicate delivery replays the
/// recorded reply.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMemberOperatorPayload {
    pub agent_identity: String,
    /// Materialized-member generation that originated this request. Required
    /// (no default): delayed requests from a superseded residency must not be
    /// rebound to the current authority generation.
    pub requester_generation: u64,
    /// Materialization fence paired with `requester_generation`. Required so
    /// a delayed request cannot cross a same-generation superseding fence.
    pub requester_fence_token: u64,
    /// Host identity that originated this request. Binding-generation
    /// counters are host-local, so generation equality without this field
    /// would permit an A:G1 request to match a migrated B:G1 residency.
    pub requester_host_id: String,
    /// Exact host-binding generation that originated the request. Host
    /// replacement can revive the same durable member generation, fence,
    /// session, and peer key, so this is an independent ABA fence.
    pub requester_host_binding_generation: u64,
    /// Exact host-resident member session that originated the request.
    /// Required: a host rematerialization can reuse member generation/fence
    /// while replacing the runtime session.
    pub requester_member_session_id: String,
    pub request_id: String,
    pub op: MemberOperatorOp,
    pub protocol_version: BridgeProtocolVersion,
}

/// Closed vocabulary covering exactly the twelve member operator tools
/// (§15.10). Each variant carries the tool's SERIALIZABLE arg surface;
/// non-portable args (per-spawn external tools) have no field at all.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberOperatorOp {
    /// Boxed: the spawn spec dwarfs the id-only sibling ops.
    SpawnMember(Box<MemberOperatorSpawnSpec>),
    SpawnManyMembers {
        specs: Vec<MemberOperatorSpawnSpec>,
    },
    RetireMember {
        member_id: String,
    },
    ForceCancelMember {
        member_id: String,
    },
    MemberStatus {
        member_id: String,
    },
    WireMembers {
        member_id: String,
        peer_member_id: String,
    },
    UnwireMembers {
        member_id: String,
        peer_member_id: String,
    },
    ListMembers,
    MobListFlows,
    MobRunFlow {
        flow_id: String,
        /// Opaque flow-param pass-through (same carrier class as
        /// `MobFlowRunParams.params`), string-enveloped for `Eq`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<WireOpaqueJson>,
    },
    MobFlowStatus {
        run_id: String,
    },
    MobCancelFlow {
        run_id: String,
    },
}

/// Serializable spawn arg surface for the member-originated spawn ops.
///
/// O3 no-laundering rule: `requested_tool_access_policy_present` (did the
/// caller ask for a policy at all) and `resolved_tool_access_policy` (the
/// resolved, never-`Inherit` policy) are SEPARATE facts — the first is
/// required so absence cannot be laundered into "not requested".
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberOperatorSpawnSpec {
    pub profile: String,
    pub member_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<meerkat_core::types::ContentInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    /// Single launch carrier for this op (the legacy
    /// `resume_bridge_session_id`/`resume_session_id` tool aliases are not
    /// wire vocabulary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_mode: Option<WireMemberLaunchMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_wire_parent: Option<bool>,
    /// Requested placement host ref (comms `PeerId` string); `None` defaults
    /// to the requesting member's host per §7.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<String>,
    pub requested_tool_access_policy_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_tool_access_policy: Option<WireResolvedToolAccessPolicy>,
}

/// Reply to a `MemberOperatorRequest`. Replayed verbatim on `request_id`
/// duplicates.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberOperatorReply {
    pub request_id: String,
    pub outcome: MemberOperatorOutcome,
}

/// Outcome of one member-originated operator op.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberOperatorOutcome {
    /// Sanctioned opaque-JSON exception: `result` is the tool output
    /// echoed back to the calling agent — surface JSON, not a semantic
    /// fact anything branches on. It rides [`WireOpaqueJson`] (never a
    /// bare `serde_json::Value`) and is parsed only by the requesting
    /// member's forwarding dispatcher. Everything semantic (admission,
    /// rejection cause) is typed.
    Completed { result: WireOpaqueJson },
    Rejected {
        cause: BridgeRejectionCause,
        reason: String,
    },
}

/// Opaque JSON carried as its serialized string form (the
/// `OpaqueProviderBody` precedent, transparent flavor).
///
/// Used where the wire must ferry surface JSON through the bridge `Eq`
/// chain: `serde_json::Value` is not `Eq`, and `RawValue` cannot decode
/// through `serde_json::from_value` (which [`decode_bridge_command`] and
/// every Value-based consumer rely on). Equality is byte equality of the
/// stored string — producers serialize exactly once via
/// [`WireOpaqueJson::from_value`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireOpaqueJson(String);

impl WireOpaqueJson {
    /// Serialize a JSON value into its canonical envelope form.
    /// (`Value::to_string` is infallible.)
    pub fn from_value(value: &serde_json::Value) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse the envelope back into a JSON value; fails typed on a
    /// hand-built envelope that is not valid JSON.
    pub fn to_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// V4 — turn directive on DeliverMemberInput (§18.10)
// ---------------------------------------------------------------------------

/// Flow-turn directive attached to a delivery (absent-omitted; fails closed
/// on pre-V4 receivers via their `deny_unknown_fields` boundary).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeTurnDirective {
    pub correlation: BridgeTurnCorrelation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_overlay: Option<meerkat_core::service::PublicTurnToolOverlay>,
}

/// Exact cross-host member residency addressed by one directed delivery.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMemberIncarnation {
    pub mob_id: String,
    pub agent_identity: String,
    pub host_id: String,
    /// Exact host authority generation for this residency. A replacement
    /// host may revive the same session, member generation, fence, and key;
    /// this independent field closes that cross-host ABA.
    pub binding_generation: u64,
    pub member_session_id: String,
    pub generation: u64,
    pub fence_token: u64,
}

/// Flow-step correlation for a directed turn.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeTurnCorrelation {
    pub run_id: String,
    pub step_id: String,
}

/// Terminal outcome record for a directed turn, delivered as a sidecar on
/// `MemberEventsPage` (the event envelope rows themselves stay unchanged —
/// §7 envelope freeze).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeTurnOutcomeRecord {
    pub input_id: String,
    pub generation: u64,
    /// Materialization fence paired with `generation`. A same-generation
    /// higher-fence residency must not accept an older watcher's terminal.
    pub fence_token: u64,
    /// Durable `StoredEvent.seq` of the terminal event for this turn.
    pub terminal_seq: u64,
    pub outcome: WireFlowTurnOutcome,
}

/// Exact acknowledgement key for one consumed directed-turn terminal.
///
/// Agent identity and mob id are implied by the addressed member session;
/// generation + fence keep a reused input id from pruning a later residency.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeTurnOutcomeAck {
    pub generation: u64,
    pub fence_token: u64,
    pub input_id: String,
}

/// The seven flow-turn terminals plus channel-close (§18.1/§18.11). Closed.
///
/// Failure details are structured because the member host may have to retain
/// a UTF-8-safe prefix to keep one durable journal row within its protocol
/// bound. Consumers can distinguish that representation compaction from the
/// original failure itself without parsing prose.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFlowTurnOutcome {
    RunCompleted,
    ExtractionSucceeded,
    ExtractionFailed { detail: WireFlowFailureDetail },
    RunFailed { detail: WireFlowFailureDetail },
    InteractionComplete,
    InteractionCallbackPending,
    InteractionFailed { detail: WireFlowFailureDetail },
    ChannelClosed,
}

/// Presentation detail for a failed directed turn.
///
/// `original_utf8_bytes` always names the byte length of the complete source
/// string. When `truncated` is true, `text` is the largest UTF-8 prefix the
/// member host could retain while keeping the complete
/// [`BridgeTurnOutcomeRecord`] within the durable protocol bound.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireFlowFailureDetail {
    pub text: String,
    pub original_utf8_bytes: u64,
    pub truncated: bool,
}

impl WireFlowFailureDetail {
    /// Construct an unabridged failure detail. Journal-bound compaction is a
    /// member-host responsibility because it depends on the complete record
    /// framing (including the canonical input id).
    #[must_use]
    pub fn complete(text: String) -> Self {
        Self {
            original_utf8_bytes: u64::try_from(text.len()).unwrap_or(u64::MAX),
            text,
            truncated: false,
        }
    }
}

// ---------------------------------------------------------------------------
// V4 — host binding descriptor (§7.2, plane a)
// ---------------------------------------------------------------------------

/// Marker discriminant for the host binding descriptor file.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireHostBindingDescriptorKind {
    Host,
}

/// Host flavor of the `--comms-binding-out` descriptor: what `rkat mob host`
/// writes for the operator to hand to the controlling mob. Mirrors the
/// member descriptor's shape (typed Ed25519 identity, never a raw peer id)
/// with `kind: "host"` and the optional advertised live endpoint (DL5/DL6).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireHostBindingDescriptor {
    pub kind: WireHostBindingDescriptorKind,
    pub address: String,
    pub identity: WireTrustedPeerIdentity,
    pub bootstrap_token: BridgeBootstrapToken,
    /// Advertised ws/wss live base URL; `None` = live-incapable host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_endpoint: Option<String>,
}

// ---------------------------------------------------------------------------
// Reply envelope
// ---------------------------------------------------------------------------

/// A typed reply from a member runtime (or mob host daemon) back to the
/// supervisor, and — for `MemberOperatorReply` — from the controlling host
/// back to a member's host.
///
/// Not `PartialEq`/`Eq`: `MemberHistoryPage` carries `WireSessionMessage`
/// rows and `MemberEventsPage` carries `AgentEvent` envelopes; both ride
/// serialized-form equality adapters (`WireHistoryRow` / `WireEventRow`) so
/// the reply chain keeps the `Eq` the comms envelope enums derive over it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeReply {
    BindMember(BridgeBindResponse),
    Ack(BridgeAck),
    Observation(BridgeObservationResponse),
    Delivery(BridgeDeliveryResponse),
    TrackedInputCancelled(BridgeTrackedInputCancelResponse),
    Retire(BridgeRetireResponse),
    Destroy(BridgeDestroyResponse),
    /// Observation of a previously submitted supervisor-rotation operation.
    /// Submission is one-way and never produces this reply directly.
    SupervisorRotation(BridgeSupervisorRotationObservation),
    Rejected {
        cause: BridgeRejectionCause,
        reason: String,
    },
    // --- V4 replies ---
    BindHost(BridgeHostBindResponse),
    HostRebound(BridgeHostReboundResponse),
    HostRevoked(BridgeHostRevokedResponse),
    MemberHistoryPage(BridgeMemberHistoryPage),
    MemberEventsPage(BridgeMemberEventsPage),
    MemberMaterialized(BridgeMaterializedResponse),
    MemberReleased(BridgeMemberReleasedResponse),
    HostStatus(BridgeHostStatusResponse),
    MemberLiveChannelOpened(BridgeLiveOpenedResponse),
    MemberLiveChannelClosed {
        status: LiveCloseStatus,
    },
    MemberLiveChannelStatusReport {
        channel_id: String,
        status: WireLiveAdapterStatus,
    },
    MemberLiveChannelControlled(BridgeLiveControlledResponse),
    MemberOperatorReply(MemberOperatorReply),
}

/// Response to `OpenMemberLiveChannel`.
///
/// The payload field is named `open`, not `result`: this enum is
/// internally tagged on `result`, so the plan's literal
/// `{ result: LiveOpenResult }` field shape is unrepresentable (serde
/// rejects a variant field that collides with the internal tag). The
/// embedded [`LiveOpenResult`] itself is verbatim so the owning host's
/// absolute URL + token round-trip untouched.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLiveOpenedResponse {
    pub open: LiveOpenResult,
}

/// Response to `ControlMemberLiveChannel` — same `result`-tag collision
/// rename as [`BridgeLiveOpenedResponse`]; the per-verb outcome nests
/// under `outcome`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLiveControlledResponse {
    pub outcome: BridgeLiveControlOutcome,
}

/// Response to `BindHost` (mirror of `BindMember`'s bind response, host
/// flavor).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostBindResponse {
    /// Canonical host `PeerId`; transport public-key bytes stay out.
    pub host_peer_id: String,
    pub binding_generation: u64,
    pub address: String,
    pub capabilities: BridgeCapabilities,
    /// Advertised ws/wss live base URL; `None` = live-incapable host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_endpoint: Option<String>,
}

/// Response to `RebindHost` (§7.2 step 4). Capabilities are re-declared in
/// full (restart-truthfulness, §14.5): the controlling host REPLACES its
/// recorded capability facts with these, and an absent `live_endpoint`
/// clears the recorded live capability.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostReboundResponse {
    /// Canonical host `PeerId`; transport public-key bytes stay out.
    pub host_peer_id: String,
    pub binding_generation: u64,
    pub capabilities: BridgeCapabilities,
    /// Advertised ws/wss live base URL; `None` = live-incapable host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_endpoint: Option<String>,
}

/// Durable success receipt for [`BridgeCommand::RevokeHost`].
///
/// The host returns this only after every recorded member residency has been
/// quiesced, its active live channel has been proven absent (or exactly
/// closed), the session has been disposed/deregistered, and the active binding
/// row has been atomically deleted with its replay receipt. All fields are host-produced
/// or machine-echoed and are validated by the controlling host before it
/// clears its own authority record.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostRevokedResponse {
    pub host_peer_id: String,
    pub mob_id: String,
    pub epoch: u64,
    pub binding_generation: u64,
    /// Host-side materialized identities actually disposed by the terminal.
    /// Membership certifies the owning disposal arc proved that identity's
    /// live channel absent before the revocation receipt committed. Exact
    /// reply-loss retries replay this list verbatim from the durable receipt.
    pub released_members: Vec<String>,
}

/// Transcript snapshot page for a remote member. Row type is the canonical
/// wire transcript row (`WireSessionMessage`) — the same rows the local
/// `SessionHistoryPage` projection produces — via the [`WireHistoryRow`]
/// equality adapter.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMemberHistoryPage {
    /// Member generation the page was read at; a bump means the visible
    /// generation window changed (the underlying session may be reused).
    pub generation: u64,
    pub page: WireMemberHistoryPageBody,
}

/// One page of a remote member's durable event stream.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMemberEventsPage {
    pub generation: u64,
    /// Materialization fence paired with `generation`. Event/fold consumers
    /// must bind both values: a fence can rotate without changing the
    /// generation or runtime id.
    pub fence_token: u64,
    /// Durable event rows carrying BOTH the owning host's exact
    /// `StoredEvent.seq` and its event envelope. Durable sequence values are
    /// not necessarily contiguous, so consumers must never infer them from
    /// vector position or `next_seq`.
    pub events: Vec<WireEventRow>,
    /// Exact read floor resolved by the owning host for this page. This may
    /// be greater than the requested cursor after a same-session generation
    /// cutover, where older durable rows belong to the prior generation.
    /// Consumers validate both the first row and an empty page's `next_seq`
    /// against this host-resolved floor instead of treating the requested
    /// cursor as the effective generation boundary.
    pub from_seq: u64,
    /// Cursor seq to resume from (durable `StoredEvent.seq` domain).
    pub next_seq: u64,
    /// Highest durable seq the owning host has recorded; with `next_seq`
    /// this lets consumers detect `StaleCursor` overruns typed.
    pub watermark: u64,
    /// Bounded page of unacknowledged flow-turn terminal rows (§18.10). The
    /// consumer acknowledges these exact `(generation, input_id)` keys on
    /// its next poll; until then the same rows remain replayable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn_outcomes: Vec<BridgeTurnOutcomeRecord>,
    /// `true` when this page contains every currently retained,
    /// unacknowledged outcome for the member generation. A delayed commit may
    /// still appear on a later poll; this flag is page completeness, not a
    /// terminal watermark.
    pub outcomes_complete: bool,
}

/// One durable event row crossing the supervisor bridge.
///
/// `durable_seq` is the owning host's exact `StoredEvent.seq`; it is separate
/// from `envelope.seq`, which belongs to the event source's stream domain.
/// `AgentEvent` deliberately derives no `PartialEq`, but the bridge reply
/// chain must be `Eq` (the comms envelope enums derive it), so envelope
/// equality is semantic JSON equality of the frozen serialized form.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireEventRow {
    pub durable_seq: u64,
    pub envelope: meerkat_core::EventEnvelope<meerkat_core::AgentEvent>,
}

impl PartialEq for WireEventRow {
    fn eq(&self, other: &Self) -> bool {
        if self.durable_seq != other.durable_seq {
            return false;
        }
        match (
            serde_json::to_value(&self.envelope),
            serde_json::to_value(&other.envelope),
        ) {
            (Ok(a), Ok(b)) => a == b,
            // Unreachable for envelope rows (their serialization is
            // infallible); kept fail-closed rather than laundering a
            // serialize error into equality.
            _ => false,
        }
    }
}

impl Eq for WireEventRow {}

/// Response to `MaterializeMember`. Replayed verbatim on idempotency-tuple
/// duplicates, including the recorded `launch_outcome`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMaterializedResponse {
    /// Member Ed25519 public key (`ed25519:<base64>` form); the private key
    /// never leaves the member host.
    pub member_pubkey: String,
    pub member_peer_id: String,
    pub advertised_address: String,
    pub session_id: String,
    /// Echo of the request digest — `CommitSpawnMembership` requires exact
    /// match with the authorized resolved digest.
    pub spec_digest: String,
    pub engine_version: String,
    pub launch_outcome: MaterializeLaunchOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_auth_binding: Option<WireAuthBindingRef>,
}

/// Response to `ReleaseMember` — the disposal class is a typed fact, never
/// inferred (§19.L3/L4). Every success additionally certifies the owning
/// disposal arc proved the member's live channel absent before recording the
/// release row.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeMemberReleasedResponse {
    pub disposal: MemberSessionDisposal,
}

/// What durably happened to the member's session on release (§19.L3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposal", rename_all = "snake_case")]
pub enum MemberSessionDisposal {
    /// Mob-owned durable record archived (commit-first protocol).
    Archived,
    /// The durable terminal already held; success-class idempotent outcome.
    AlreadyArchived,
    /// Runtime retired + binding released, but no mob-owned durable archive
    /// happened — the typed cause says why, never silently.
    RuntimeReleasedOnly { cause: RuntimeReleaseCause },
}

/// Why a release was runtime-only (§19.L3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeReleaseCause {
    /// Adopted host-owned session: durable lifecycle owned elsewhere.
    HostOwnedSession,
    /// Ephemeral host (`durable_sessions=false` at bind) — capability
    /// degradation declared at bind, never silent.
    NoDurableSessions,
}

/// Response to `HostStatus`: the host's materialized inventory. Feeds
/// orphan reconciliation (`ReleaseMember` at a stale fence) and the
/// reachability projection.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostStatusResponse {
    pub members: Vec<BridgeHostMemberRecord>,
    pub capabilities: BridgeCapabilities,
}

/// One materialized-member row in a `HostStatus` reply.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHostMemberRecord {
    pub agent_identity: String,
    pub generation: u64,
    pub fence_token: u64,
    pub session_id: String,
    pub spec_digest: String,
    pub healthy: bool,
}

/// Per-verb result of `ControlMemberLiveChannel` — mirrors the typed
/// statuses the local live command results carry.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeLiveControlOutcome {
    CommitInput { status: LiveCommitInputStatus },
    Interrupt { status: LiveInterruptStatus },
    Truncate { status: LiveTruncateStatus },
    Refresh { status: LiveRefreshStatus },
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
            Self::Typed { cause, .. } => Some(cause.clone()),
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
///
/// Not `Copy` since V4: several causes carry typed detail payloads.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    // --- V4 multi-host causes (§7) ---
    /// The command carried a superseded `(generation, fence_token)` tuple.
    StaleFence,
    /// The event cursor overran the host's retained window; the reply
    /// carries the current position so the poller can restart cleanly.
    StaleCursor { watermark: u64, generation: u64 },
    /// One durable event envelope cannot fit the bridge reply budget. The
    /// poller must record the typed gap and resume at `next_seq`; retrying the
    /// same cursor would livelock forever on the poison row.
    OversizedEvent {
        generation: u64,
        durable_seq: u64,
        next_seq: u64,
        encoded_bytes: u64,
        max_bytes: u64,
    },
    /// One transcript row cannot fit the bridge reply budget. The request
    /// fails at the exact row index instead of returning an empty page that
    /// livelocks continuation or silently dropping fork context.
    HistoryRowTooLarge {
        index: u64,
        encoded_bytes: u64,
        max_bytes: u64,
    },
    /// The addressed member/host is transiently unable to serve the
    /// command (observation degrades typed, never quiet).
    Unavailable,
    /// Plane-(b) control-scope denial (A9).
    ScopeDenied {
        required: WireControlScope,
        presented: Vec<WireControlScope>,
    },
    /// The idempotency tuple was replayed with a different spec digest
    /// (A12 — one key never names two builds), or the received spec does
    /// not hash to the digest the command carried.
    SpecDigestMismatch,
    /// The host could not build the member; the typed cause taxonomy is
    /// §14.4's table.
    MaterializeBuildRejected { cause: MemberBuildRejection },
    /// Model unknown to the host's registry with no provider override.
    ModelUnresolvable { model: String },
    /// The named realm/binding does not resolve on the host's realm chain.
    AuthBindingUnresolvable { realm: String, binding: String },
    /// A declared MCP stdio server's command is not present on the host.
    McpCommandMissing { server: String },
    /// The host's realm persistence backend is unavailable.
    RealmBackendUnavailable,
    /// A `required_env_keys` entry cannot be satisfied on the host (values
    /// never travel — R5).
    EnvKeyMissing { key: String },
    /// The host reports a different engine version than the one recorded
    /// at bind; feeds rebind.
    HostEngineVersionChanged { bound: String, reported: String },
    // --- V4 live-channel causes (§16.4) ---
    /// The member's model has no realtime capability.
    ModelNotRealtime { model: String, provider: String },
    /// No live adapter is available for the member's provider.
    LiveAdapterUnavailable { provider: String },
    /// The host has no live transport configured/advertised.
    LiveTransportUnavailable,
    /// The member already has a bound live channel.
    LiveChannelAlreadyBound,
    /// The addressed live channel does not exist.
    LiveChannelNotFound,
    /// The requested live transport is not supported by the host.
    LiveTransportUnsupported { requested: String },
    // --- V4 launch causes (§19.L1) ---
    /// `Resume` named a session with no live runtime and no resumable snapshot.
    /// Archived/Retired rows are intentionally non-resumable and use this same
    /// stable cause so the controller advances generation instead of retrying
    /// the absorbing session tuple.
    ResumeSessionNotFound,
    /// A capability the launch path requires is absent on the host (e.g.
    /// interaction-event injector for autonomous mode, `durable_sessions`
    /// for snapshot restore).
    CapabilityMissing { capability: String },
    /// The launch mode reached admission but is not supported there.
    LaunchModeUnsupported,
    /// A `Resume` session id is bound to a different placement than the
    /// spawn requested (controlling-side early check).
    LaunchModePlacementMismatch,
    /// The requested durable session is already owned by another
    /// `(mob_id, agent_identity)` row on this host. The host rejects before
    /// rebinding any live runtime or recording an alias.
    SessionOwnershipConflict { session_id: String },
}

/// Typed "host cannot build the member" taxonomy (§14.4). Closed — every
/// owning-host build failure maps onto one of these, never a generic
/// failure. Externally tagged (like the parent cause enum) so the
/// `kind`-named detail fields don't collide with a tag key.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberBuildRejection {
    /// Model unknown to the host registry, no provider override.
    UnknownProviderForModel { model: String },
    /// Explicit binding's realm/binding absent or invalid on the host's
    /// realm chain.
    BindingUnresolvable { kind: ConnectionTargetErrorKind },
    /// Credential material missing / needs login / expired / lease-gated.
    ProviderAuth { kind: meerkat_core::AuthErrorKind },
    /// Self-hosted alias not configured on the host.
    SelfHostedServerMissing { server_id: String },
}

/// Wire projection of `meerkat_core::connection::ConnectionTargetError`'s
/// variant set (closed; kinds only — the parameters stay in the diagnostic
/// `reason` string).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionTargetErrorKind {
    MissingRealm,
    UnknownRealm,
    MissingDefaultBinding,
    InvalidRealmId,
    InvalidBindingId,
    RealmConfigInvalid,
    BindingInvalid,
    ProviderMismatch,
    RealmChain,
}

// The recoverable-vs-fatal recovery verdict for a `BridgeRejectionCause` is a
// MobMachine-owned semantic fact (mob supervisor-authority recovery lifecycle),
// decided inside the canonical MobMachine via the
// `ClassifyBridgeRejectionRecovery` input and mirrored by the mob shell. The
// wire reply carries only the raw cause; no protocol-level recoverability class
// is reduced here.

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

/// Stable identifier for one supervisor-rotation operation.
///
/// The UUID is validated at every string/serde ingress boundary. Keeping the
/// inner value private prevents callers from constructing an unchecked raw
/// string identifier that could diverge between submission, observation, and
/// durable receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct SupervisorRotationOperationId(
    #[cfg_attr(feature = "schema", schemars(with = "String"))] uuid::Uuid,
);

impl SupervisorRotationOperationId {
    /// Create a fresh operation identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Construct an operation identifier from a typed UUID.
    #[must_use]
    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    /// Return the underlying typed UUID.
    #[must_use]
    pub const fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for SupervisorRotationOperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SupervisorRotationOperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for SupervisorRotationOperationId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        uuid::Uuid::parse_str(value).map(Self)
    }
}

impl From<uuid::Uuid> for SupervisorRotationOperationId {
    fn from(value: uuid::Uuid) -> Self {
        Self::from_uuid(value)
    }
}

impl From<SupervisorRotationOperationId> for uuid::Uuid {
    fn from(value: SupervisorRotationOperationId) -> Self {
        value.0
    }
}

/// One-way submission of a durable supervisor-rotation operation.
///
/// Authentication comes from the signed peer-message envelope. The payload
/// therefore carries only the stable operation identity and requested target;
/// it does not smuggle a second sender claim into the command body.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeSupervisorRotationSubmit {
    pub operation_id: SupervisorRotationOperationId,
    pub target: BridgePeerSpec,
    pub target_epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
}

/// Request to observe a previously submitted supervisor rotation.
///
/// The observer identity and epoch are explicit proof inputs that the member
/// validates against the authenticated request sender and its current or
/// pending authority state.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeSupervisorRotationObserve {
    pub operation_id: SupervisorRotationOperationId,
    pub observer: BridgePeerSpec,
    pub observer_epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
}

/// Durable phase exposed while a supervisor rotation is incomplete.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeSupervisorRotationPendingPhase {
    PreviousRevokePending,
    NextPublishPending,
}

/// Exact target identity committed to a supervisor-rotation operation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeSupervisorRotationTargetReceipt {
    pub target: BridgePeerSpec,
    pub target_epoch: u64,
}

/// Operation-correlated receipt used by terminal supervisor-rotation states.
///
/// It carries the exact target identity and epoch, rather than only a peer id,
/// so a recovered observer can project the same durable authority without
/// joining against mutable peer-directory state.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeSupervisorRotationOperationReceipt {
    pub operation_id: SupervisorRotationOperationId,
    pub target: BridgeSupervisorRotationTargetReceipt,
}

/// Stable machine-readable cause for a terminally rejected rotation.
///
/// Consumers branch on this value. Human-readable details belong only in the
/// sibling `reason` field of [`BridgeSupervisorRotationRejectionReceipt`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeSupervisorRotationRejectionCause {
    OperationConflict,
    InvalidTarget,
    StaleTargetEpoch,
    SenderMismatch,
    UnsupportedProtocolVersion,
    Internal,
}

/// Durable receipt for a terminally rejected supervisor rotation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeSupervisorRotationRejectionReceipt {
    pub operation: BridgeSupervisorRotationOperationReceipt,
    pub cause: BridgeSupervisorRotationRejectionCause,
    /// Operator-facing diagnostic only; callers must not parse it.
    pub reason: String,
}

/// Durable state of a known supervisor-rotation operation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeSupervisorRotationState {
    Pending {
        operation: BridgeSupervisorRotationOperationReceipt,
        phase: BridgeSupervisorRotationPendingPhase,
    },
    Completed {
        receipt: BridgeSupervisorRotationOperationReceipt,
    },
    Rejected {
        receipt: BridgeSupervisorRotationRejectionReceipt,
    },
}

/// Observation result for a supervisor-rotation operation.
///
/// `NotFound` is an observation outcome, never a durable rejected state: an
/// observer may race the one-way submission before the member has committed
/// it and can safely retry the observation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeSupervisorRotationObservation {
    Found {
        state: BridgeSupervisorRotationState,
    },
    NotFound {
        operation_id: SupervisorRotationOperationId,
    },
}

/// Explicit hard-cancel command payload.
///
/// `InterruptMember` is the cooperative boundary-break path. This payload is
/// intentionally separate so supervisors cannot accidentally collapse boundary
/// cancellation and immediate user/session interrupt authority onto one wire
/// command. `operation_id` remains stable across transport retries, while
/// `expected_run_id` fences the level-triggered request to one exact run: a
/// delayed retry can observe that run already terminal, but must never cancel a
/// newer run that subsequently became current.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeHardCancelPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    pub operation_id: meerkat_core::ops::OperationId,
    pub expected_run_id: meerkat_core::RunId,
    pub reason: String,
}

/// Cancel one exact tracked member delivery under its full residency fence.
///
/// The host owns a lifecycle-bounded durable negative receipt for this key.
/// Once this command returns `NoEffect` or `Cancelled`, an arbitrarily delayed
/// `DeliverMemberInput` carrying the same key is deduplicated before runtime
/// admission. The explicit capability bit is required in addition to V4:
/// older V4 peers do not know this command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeTrackedInputCancelPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub expected_member: BridgeMemberIncarnation,
    pub input_id: String,
}

/// Stable terminal class for an exact tracked-input cancellation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BridgeTrackedInputCancelOutcome {
    /// The host durably fenced the key before any delivery effect was
    /// admitted. This is the only definite-no-effect certificate.
    NoEffect,
    /// A prior or racing admission was possible; the host durably fenced the
    /// key and converged its runtime effect to cancellation.
    Cancelled,
    /// The tracked turn won the race and its exact durable terminal remains
    /// available. The controller consumes this record verbatim.
    Terminal { record: BridgeTurnOutcomeRecord },
}

/// Response to [`BridgeCommand::CancelTrackedMemberInput`]. Echoing the full
/// residency and key prevents a reply from being applied to a replacement
/// member or a different kickoff.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeTrackedInputCancelResponse {
    pub expected_member: BridgeMemberIncarnation,
    pub input_id: String,
    pub outcome: BridgeTrackedInputCancelOutcome,
}

/// Cooperative boundary interrupt. Host-materialized callers carry the exact
/// residency captured at controlling-side admission; peer-only legacy callers
/// omit it. A receiver with a registered host incarnation rejects omission or
/// mismatch before enqueuing any cancel effect.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeInterruptPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_member: Option<BridgeMemberIncarnation>,
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
    /// Exact durable tracked-input cancellation. This bit is intentionally
    /// independent of the V4 protocol version because older V4 peers do not
    /// implement the command.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub tracked_input_cancel: bool,
    #[serde(default)]
    pub retire_member: bool,
    #[serde(default)]
    pub destroy_member: bool,
    #[serde(default)]
    pub wire_member: bool,
    #[serde(default)]
    pub unwire_member: bool,
    // V4 host-capability facts. All `#[serde(default)]` so a V2/V3-era
    // capabilities payload (fields absent) still decodes. Field names stay
    // aligned with the DSL `HostCapabilityFlags` single enumeration (§6.1)
    // — this struct is its bind-ceremony wire carrier, not a second owner.
    /// Host persists sessions durably (event-sourced); `false` means
    /// releases resolve `RuntimeReleasedOnly { NoDurableSessions }`.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub durable_sessions: bool,
    /// Host can run `AutonomousHost`-mode members.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub autonomous_members: bool,
    /// Host has a semantic memory store available.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub memory_store: bool,
    /// Host can connect declarative MCP servers.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub mcp: bool,
    /// Engine version string; `""` = not reported (pre-V4 peer).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub engine_version: String,
    /// Host forwards member approval requests to the controlling host.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub approval_forwarding: bool,
    /// Providers the host can resolve credentials for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolvable_providers: Vec<meerkat_core::Provider>,
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
            tracked_input_cancel: false,
            retire_member: false,
            destroy_member: false,
            wire_member: false,
            unwire_member: false,
            durable_sessions: false,
            autonomous_members: false,
            memory_store: false,
            mcp: false,
            engine_version: String::new(),
            approval_forwarding: false,
            resolvable_providers: Vec::new(),
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
    /// Canonical non-nil transport/idempotency UUID. For tracked completion
    /// this is also the retained sidecar key; it is not implicitly the
    /// transcript identity for untracked placed admission.
    pub input_id: String,
    /// Transcript correlation for this placed work item. Independent from
    /// `input_id`: ingress-acknowledged delivery keeps a fresh transport /
    /// idempotency key while preserving the caller's interaction identity.
    /// Turn-completed delivery intentionally uses the same canonical UUID for
    /// both because its retained sidecar is keyed by `input_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_interaction_id: Option<String>,
    pub content: meerkat_core::types::ContentInput,
    pub handling_mode: meerkat_core::types::HandlingMode,
    /// Durable delegated-objective causality for this work delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    /// Exact destination residency for host-materialized members. Required
    /// whenever the target session has a host journal; peer-only legacy
    /// deliveries may omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_member: Option<BridgeMemberIncarnation>,
    /// Host-attached injected context delivered alongside the work content.
    /// Each entry materializes on the member as a separate typed
    /// injected-context transcript message immediately before the work
    /// content, in order. Omitted when empty, so the serialized payload is
    /// byte-identical to the pre-field wire shape (see the V3 note in the
    /// `BridgeProtocolVersion` history for the cross-version posture).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context: Vec<meerkat_core::types::ContentInput>,
    /// Flow-turn directive (V4, §18.10). Absent-omitted so undirected
    /// deliveries stay byte-identical to the pre-field wire shape; a
    /// directed delivery sent to a pre-V4 receiver fails closed at its
    /// `deny_unknown_fields` boundary (never a silently undirected turn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<BridgeTurnDirective>,
    /// Explicit durable terminal custody for a plain peer-shaped delivery
    /// (V4 semantic fold). Absent means no interaction outcome is promised.
    /// This marker is mutually exclusive with `turn`, whose flow-step semantics
    /// already own tracked terminal publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_tracking: Option<BridgeOutcomeTracking>,
}

/// Requested terminal-outcome custody for a plain bridge delivery.
///
/// This is an enum rather than a boolean so future custody contracts cannot
/// silently reinterpret an old `true` value.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeOutcomeTracking {
    Interaction,
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

/// Maximum exact remote-turn outcome acknowledgements admitted in one
/// `PollMemberEvents` request. Sender and receiver share this wire ceiling;
/// receivers reject oversized batches instead of silently truncating them.
pub const BRIDGE_TURN_OUTCOME_ACK_MAX: usize = 64;

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
    /// The delivery requested tracked terminal custody the receiver cannot
    /// honor (§18.10) — either through a `turn` directive or explicit
    /// interaction tracking. Rejected typed instead of running untracked.
    TurnDirectiveUnsupported { detail: String },
    /// The member's durable, unacknowledged directed-turn outcome journal is
    /// at its hard per-member retention limit. The controller must consume
    /// and acknowledge outcomes before submitting more directed work.
    OutcomeJournalFull { retained: u32, limit: u32 },
    /// The request addressed a superseded or aliased member residency. This
    /// is checked before durable Pending, so rejection certifies no effect.
    StaleMemberIncarnation { current: BridgeMemberIncarnation },
    /// A fenced runtime admission lost its exact residency after Pending was
    /// reserved. Unlike `StaleMemberIncarnation`, the replacement may be
    /// absent (release/vacate) or may use a different session id, so both the
    /// request's expected incarnation and the optional observed replacement
    /// are carried without fabricating a `current` value.
    StaleMemberResidency {
        expected: BridgeMemberIncarnation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<BridgeMemberIncarnation>,
    },
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
            Self::TurnDirectiveUnsupported { detail } => {
                write!(f, "turn_directive_unsupported(detail={detail})")
            }
            Self::OutcomeJournalFull { retained, limit } => {
                write!(
                    f,
                    "outcome_journal_full(retained={retained}, limit={limit})"
                )
            }
            Self::StaleMemberIncarnation { current } => write!(
                f,
                "stale_member_incarnation(current={}/{}/{}/{}/{}/{}/{})",
                current.mob_id,
                current.agent_identity,
                current.host_id,
                current.binding_generation,
                current.member_session_id,
                current.generation,
                current.fence_token
            ),
            Self::StaleMemberResidency { expected, current } => write!(
                f,
                "stale_member_residency(expected={}/{}/{}/{}/{}/{}/{}, current={current:?})",
                expected.mob_id,
                expected.agent_identity,
                expected.host_id,
                expected.binding_generation,
                expected.member_session_id,
                expected.generation,
                expected.fence_token
            ),
            Self::Internal { detail } => write!(f, "internal(detail={detail})"),
        }
    }
}

/// Full response to a delivery command.
///
/// The bridge delivery wire contract advertises only the accept-boundary
/// outcome (`outcome`) and the canonical input id. There is deliberately no
/// turn-completion payload here: a `DeliverMemberInput` command acknowledges
/// admission of one logical input, not the eventual turn result. Turn
/// completion is observed through the runtime completion seam
/// (`CompletionFeed` / `CompletionOutcome`), never re-derived onto this
/// delivery acknowledgement — advertising a completion field that every
/// producer fills with `None` was pure schema theater and has been removed
/// so the wire shape matches what is actually delivered.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryResponse {
    pub input_id: String,
    pub canonical_input_id: Option<String>,
    pub outcome: BridgeDeliveryOutcome,
}

/// Generated MobMachine peer overlay handoff carried with peer wiring commands.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgeMobPeerOverlayHandoff {
    pub recipient_peer_id: String,
    pub topology_epoch: u64,
    pub peer_specs: Vec<BridgePeerSpec>,
}

/// Peer wiring command payload.
///
/// `mob_peer_overlay` is present from protocol V3 onward. It is
/// `#[serde(default, skip_serializing_if)]` so that (a) a V2 wiring payload
/// emitted by a pre-overlay peer still deserializes here as `None` instead of
/// erroring on a missing field, and (b) a V3 payload remains byte-identical on
/// the wire to the pre-Option shape (the field is always `Some` when emitted by
/// V3 senders). A `None` overlay on a wiring command is rejected by the
/// receiver with a typed `UnsupportedProtocolVersion` cause.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BridgePeerWiringPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: BridgeProtocolVersion,
    pub peer_spec: BridgePeerSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_peer_overlay: Option<BridgeMobPeerOverlayHandoff>,
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

    fn sample_member_incarnation() -> BridgeMemberIncarnation {
        BridgeMemberIncarnation {
            mob_id: "mob-1".to_string(),
            agent_identity: "worker-1".to_string(),
            host_id: "host-1".to_string(),
            binding_generation: 1,
            member_session_id: "session-1".to_string(),
            generation: 1,
            fence_token: 3,
        }
    }

    fn sample_hard_cancel_payload() -> BridgeHardCancelPayload {
        BridgeHardCancelPayload {
            supervisor: sample_peer_spec(),
            epoch: 42,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_member: sample_member_incarnation(),
            operation_id: meerkat_core::ops::OperationId(uuid::Uuid::from_u128(0xCA11)),
            expected_run_id: meerkat_core::RunId::from_uuid(uuid::Uuid::from_u128(0xA11CE)),
            reason: "test hard cancel".to_string(),
        }
    }

    fn sample_tracked_input_cancel_payload() -> BridgeTrackedInputCancelPayload {
        BridgeTrackedInputCancelPayload {
            supervisor: sample_peer_spec(),
            epoch: 42,
            protocol_version: BridgeProtocolVersion::V4,
            expected_member: sample_member_incarnation(),
            input_id: uuid::Uuid::from_u128(0xCA11CE1).to_string(),
        }
    }

    fn sample_interrupt_payload() -> BridgeInterruptPayload {
        BridgeInterruptPayload {
            supervisor: sample_peer_spec(),
            epoch: 42,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_member: None,
        }
    }

    fn sample_wiring_payload() -> BridgePeerWiringPayload {
        let peer_spec = BridgePeerSpec {
            name: "member-b".to_string(),
            peer_id: "peer-xyz".to_string(),
            address: "tcp://127.0.0.1:7001".to_string(),
            pubkey: [0u8; 32],
        };
        BridgePeerWiringPayload {
            supervisor: sample_peer_spec(),
            epoch: 7,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            mob_peer_overlay: Some(BridgeMobPeerOverlayHandoff {
                recipient_peer_id: sample_peer_spec().peer_id,
                topology_epoch: 11,
                peer_specs: vec![peer_spec.clone()],
            }),
            peer_spec,
        }
    }

    fn valid_trusted_peer(name: &str, seed: u8, address: &str) -> TrustedPeerDescriptor {
        let pubkey = [seed; 32];
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name.to_string(),
            PeerId::from_ed25519_pubkey(&pubkey).as_str(),
            pubkey,
            address,
        )
        .expect("valid trusted peer descriptor")
    }

    // -----------------------------------------------------------------------
    // 1. BridgeCommand JSON round-trip — one subtest per variant.
    // -----------------------------------------------------------------------
    //
    // Round-trip correctness is asserted on the `serde_json::Value` form
    // (before and after a decode/encode cycle): equal JSON values pin the
    // WIRE shape, which is the contract under test — `PartialEq` on the
    // enum would only assert in-memory equality.

    fn sample_supervisor_rotation_operation_id() -> SupervisorRotationOperationId {
        "87de75b9-9bce-4c28-a022-7de5c9d7d480"
            .parse()
            .expect("valid supervisor rotation operation id")
    }

    fn sample_supervisor_rotation_target() -> BridgePeerSpec {
        valid_trusted_peer("next-supervisor", 9, "tcp://127.0.0.1:7010").into()
    }

    fn sample_supervisor_rotation_submit() -> BridgeSupervisorRotationSubmit {
        BridgeSupervisorRotationSubmit {
            operation_id: sample_supervisor_rotation_operation_id(),
            target: sample_supervisor_rotation_target(),
            target_epoch: 43,
            protocol_version: BridgeProtocolVersion::V4,
        }
    }

    fn sample_supervisor_rotation_receipt() -> BridgeSupervisorRotationOperationReceipt {
        BridgeSupervisorRotationOperationReceipt {
            operation_id: sample_supervisor_rotation_operation_id(),
            target: BridgeSupervisorRotationTargetReceipt {
                target: sample_supervisor_rotation_target(),
                target_epoch: 43,
            },
        }
    }

    #[test]
    fn supervisor_rotation_operation_id_validates_uuid_at_json_ingress() {
        let operation_id = sample_supervisor_rotation_operation_id();
        let value = serde_json::to_value(operation_id).expect("serialize operation id");
        assert_eq!(value, json!("87de75b9-9bce-4c28-a022-7de5c9d7d480"));

        let decoded: SupervisorRotationOperationId =
            serde_json::from_value(value).expect("decode operation id");
        assert_eq!(decoded, operation_id);
        assert!(
            serde_json::from_value::<SupervisorRotationOperationId>(json!("not-a-uuid")).is_err(),
            "an unchecked string must not cross the operation-id boundary"
        );
    }

    #[test]
    fn supervisor_rotation_submit_uses_closed_one_way_delivery_envelope() {
        let delivery =
            BridgeSupervisorDelivery::SubmitSupervisorRotation(sample_supervisor_rotation_submit());
        assert_eq!(delivery.protocol_version(), BridgeProtocolVersion::V4);

        let value = serde_json::to_value(&delivery).expect("serialize delivery");
        assert_eq!(value["delivery"], json!("submit_supervisor_rotation"));
        assert_eq!(
            value["operation_id"],
            json!("87de75b9-9bce-4c28-a022-7de5c9d7d480")
        );
        assert_eq!(value["target_epoch"], json!(43));
        assert_eq!(value["protocol_version"], json!(4));

        let decoded: BridgeSupervisorDelivery =
            serde_json::from_value(value).expect("decode delivery");
        assert_eq!(decoded, delivery);
    }

    #[test]
    fn supervisor_rotation_delivery_does_not_admit_request_commands_or_unknown_fields() {
        let observe = BridgeCommand::ObserveSupervisorRotation(BridgeSupervisorRotationObserve {
            operation_id: sample_supervisor_rotation_operation_id(),
            observer: sample_peer_spec(),
            observer_epoch: 42,
            protocol_version: BridgeProtocolVersion::V4,
        });
        let observe_value = serde_json::to_value(observe).expect("serialize observe command");
        assert!(
            serde_json::from_value::<BridgeSupervisorDelivery>(observe_value).is_err(),
            "a request command must never decode as a one-way supervisor delivery"
        );

        let mut delivery_value = serde_json::to_value(
            BridgeSupervisorDelivery::SubmitSupervisorRotation(sample_supervisor_rotation_submit()),
        )
        .expect("serialize delivery");
        delivery_value
            .as_object_mut()
            .expect("delivery object")
            .insert("user_content".to_string(), json!(true));
        assert!(
            serde_json::from_value::<BridgeSupervisorDelivery>(delivery_value).is_err(),
            "the delivery envelope must fail closed on arbitrary user fields"
        );
    }

    #[test]
    fn supervisor_rotation_observe_is_v4_request_command() {
        let command = BridgeCommand::ObserveSupervisorRotation(BridgeSupervisorRotationObserve {
            operation_id: sample_supervisor_rotation_operation_id(),
            observer: sample_peer_spec(),
            observer_epoch: 42,
            protocol_version: BridgeProtocolVersion::V4,
        });
        assert_eq!(command.protocol_version(), BridgeProtocolVersion::V4);
        assert_command_round_trip(&command);

        let value = serde_json::to_value(command).expect("serialize observe command");
        assert_eq!(value["command"], json!("observe_supervisor_rotation"));
        assert_eq!(value["observer_epoch"], json!(42));
        assert_eq!(value["protocol_version"], json!(4));
    }

    #[test]
    fn supervisor_rotation_pending_and_not_found_are_observation_only_outcomes() {
        let pending = BridgeReply::SupervisorRotation(BridgeSupervisorRotationObservation::Found {
            state: BridgeSupervisorRotationState::Pending {
                operation: sample_supervisor_rotation_receipt(),
                phase: BridgeSupervisorRotationPendingPhase::PreviousRevokePending,
            },
        });
        let pending_value = serde_json::to_value(&pending).expect("serialize pending reply");
        assert_eq!(pending_value["result"], json!("supervisor_rotation"));
        assert_eq!(pending_value["outcome"], json!("found"));
        assert_eq!(pending_value["state"]["status"], json!("pending"));
        assert_eq!(
            pending_value["state"]["operation"]["operation_id"],
            json!("87de75b9-9bce-4c28-a022-7de5c9d7d480")
        );
        assert_eq!(
            pending_value["state"]["phase"],
            json!("previous_revoke_pending")
        );
        let pending_round_trip: BridgeReply =
            serde_json::from_value(pending_value).expect("decode pending reply");
        assert_eq!(pending_round_trip, pending);

        let not_found =
            BridgeReply::SupervisorRotation(BridgeSupervisorRotationObservation::NotFound {
                operation_id: sample_supervisor_rotation_operation_id(),
            });
        let not_found_value = serde_json::to_value(&not_found).expect("serialize not-found reply");
        assert_eq!(not_found_value["outcome"], json!("not_found"));
        assert!(not_found_value.get("receipt").is_none());
        let not_found_round_trip: BridgeReply =
            serde_json::from_value(not_found_value).expect("decode not-found reply");
        assert_eq!(not_found_round_trip, not_found);
    }

    #[test]
    fn supervisor_rotation_terminal_receipts_pin_operation_and_exact_target() {
        let completed_receipt = sample_supervisor_rotation_receipt();
        let completed =
            BridgeReply::SupervisorRotation(BridgeSupervisorRotationObservation::Found {
                state: BridgeSupervisorRotationState::Completed {
                    receipt: completed_receipt.clone(),
                },
            });
        let completed_value = serde_json::to_value(&completed).expect("serialize completed reply");
        let receipt = &completed_value["state"]["receipt"];
        assert_eq!(
            receipt["operation_id"],
            json!("87de75b9-9bce-4c28-a022-7de5c9d7d480")
        );
        assert_eq!(receipt["target"]["target_epoch"], json!(43));
        assert_eq!(
            receipt["target"]["target"]["peer_id"],
            json!(sample_supervisor_rotation_target().peer_id)
        );
        let completed_round_trip: BridgeReply =
            serde_json::from_value(completed_value).expect("decode completed reply");
        assert_eq!(completed_round_trip, completed);

        let rejected =
            BridgeReply::SupervisorRotation(BridgeSupervisorRotationObservation::Found {
                state: BridgeSupervisorRotationState::Rejected {
                    receipt: BridgeSupervisorRotationRejectionReceipt {
                        operation: completed_receipt,
                        cause: BridgeSupervisorRotationRejectionCause::OperationConflict,
                        reason: "operation id is already bound to another target".to_string(),
                    },
                },
            });
        let rejected_value = serde_json::to_value(&rejected).expect("serialize rejected reply");
        assert_eq!(
            rejected_value["state"]["receipt"]["cause"],
            json!("operation_conflict")
        );
        assert_eq!(
            rejected_value["state"]["receipt"]["reason"],
            json!("operation id is already bound to another target")
        );
        let rejected_round_trip: BridgeReply =
            serde_json::from_value(rejected_value).expect("decode rejected reply");
        assert_eq!(rejected_round_trip, rejected);
    }

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
    fn bridge_peer_wiring_payload_carries_generated_mob_overlay_handoff() {
        let peer = valid_trusted_peer("member-b", 2, "tcp://127.0.0.1:7001");
        let external = valid_trusted_peer("external", 3, "tcp://127.0.0.1:7002");
        let payload = BridgePeerWiringPayload {
            supervisor: sample_peer_spec(),
            epoch: 7,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            peer_spec: peer.clone().into(),
            mob_peer_overlay: Some(BridgeMobPeerOverlayHandoff {
                recipient_peer_id: "recipient-peer".to_string(),
                topology_epoch: 13,
                peer_specs: vec![peer.into(), external.into()],
            }),
        };

        let value = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(value["mob_peer_overlay"]["topology_epoch"], json!(13));
        assert_eq!(
            value["mob_peer_overlay"]["recipient_peer_id"],
            json!("recipient-peer")
        );
        assert_eq!(
            value["mob_peer_overlay"]["peer_specs"]
                .as_array()
                .expect("overlay array")
                .len(),
            2
        );
        let decoded: BridgePeerWiringPayload =
            serde_json::from_value(value).expect("decode payload");
        let decoded_overlay = decoded
            .mob_peer_overlay
            .expect("V3 payload carries the overlay");
        assert_eq!(decoded_overlay.topology_epoch, 13);
        assert_eq!(decoded_overlay.peer_specs.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Cross-version wire compatibility (protocol V2 <-> V3, mob_peer_overlay).
    //
    // V3 added the `mob_peer_overlay` field to peer wiring commands. These
    // tests pin the contract that keeps a mixed-version distributed mob from
    // breaking silently or confusingly:
    //   * old (V2) -> new (V3) receiver: a wiring payload WITHOUT the overlay
    //     still deserializes (overlay -> None); the receiver rejects None with a
    //     typed cause rather than a raw missing-field serde error.
    //   * new (V3) -> old (V2) receiver: the old receiver's decode rejects the
    //     V3 `protocol_version` with a typed `UnsupportedProtocolVersion` BEFORE
    //     it ever tries to deserialize the unknown-shaped (overlay-bearing)
    //     payload, so it fails cleanly instead of on `deny_unknown_fields`.
    // -----------------------------------------------------------------------

    #[test]
    fn wire_member_v2_payload_without_overlay_decodes_as_none() {
        let mut value = serde_json::to_value(BridgeCommand::WireMember(sample_wiring_payload()))
            .expect("serialize");
        let obj = value.as_object_mut().expect("command object");
        // A pre-overlay V2 peer emits no `mob_peer_overlay`.
        obj.remove("mob_peer_overlay");
        obj.insert("protocol_version".to_string(), json!(2));
        match decode_bridge_command(value).expect("V2 wiring payload must decode on a V3 receiver")
        {
            BridgeCommand::WireMember(payload) => {
                assert_eq!(payload.protocol_version, BridgeProtocolVersion::V2);
                assert!(
                    payload.mob_peer_overlay.is_none(),
                    "a V2 wiring payload carries no overlay"
                );
            }
            other => panic!("expected WireMember, got {other:?}"),
        }
    }

    #[test]
    fn wire_member_v3_payload_round_trips_with_overlay() {
        let value = serde_json::to_value(BridgeCommand::WireMember(sample_wiring_payload()))
            .expect("serialize");
        assert_eq!(
            value["protocol_version"],
            json!(4),
            "the sample stamps CURRENT (V4); wiring itself only demands V3"
        );
        assert!(
            !value["mob_peer_overlay"].is_null(),
            "a V3 wiring payload always carries the overlay"
        );
        match decode_bridge_command(value).expect("V3 wiring payload decodes") {
            BridgeCommand::WireMember(payload) => {
                assert!(payload.mob_peer_overlay.is_some());
            }
            other => panic!("expected WireMember, got {other:?}"),
        }
    }

    #[test]
    fn wiring_command_with_unsupported_protocol_version_rejects_before_serde() {
        let mut value = serde_json::to_value(BridgeCommand::WireMember(sample_wiring_payload()))
            .expect("serialize");
        value
            .as_object_mut()
            .expect("command object")
            .insert("protocol_version".to_string(), json!(99));
        let err =
            decode_bridge_command(value).expect_err("unsupported protocol version must reject");
        assert!(
            matches!(err, BridgeCommandDecodeError::UnsupportedProtocolVersion(_)),
            "expected typed UnsupportedProtocolVersion, got {err:?}"
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
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: "input-1".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: None,
        });
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_declare_member_outbound_taint_round_trip() {
        for taint in [
            None,
            Some(meerkat_core::comms::SenderContentTaint::Clean),
            Some(meerkat_core::comms::SenderContentTaint::Tainted),
        ] {
            let cmd = BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
                supervisor: sample_peer_spec(),
                epoch: 4,
                protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                target: Some(BridgeOutboundTaintTarget::PeerOnly),
                taint,
            });
            assert_command_round_trip(&cmd);
            assert_eq!(cmd.protocol_version(), SUPERVISOR_BRIDGE_PROTOCOL_VERSION);
        }
    }

    #[test]
    fn bridge_outbound_taint_v3_legacy_shape_decodes_but_target_extension_requires_v4() {
        let legacy = BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
            supervisor: sample_peer_spec(),
            epoch: 4,
            protocol_version: BridgeProtocolVersion::V3,
            target: None,
            taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
        });
        let legacy_value = serde_json::to_value(&legacy).expect("serialize legacy V3 taint");
        assert!(
            legacy_value.get("target").is_none(),
            "legacy payload must preserve the pre-V4 omitted shape"
        );
        let decoded = decode_bridge_command(legacy_value.clone())
            .expect("omitted-target V3 payload remains decodable");
        assert_eq!(decoded, legacy);

        let mut under_versioned = legacy_value;
        under_versioned
            .as_object_mut()
            .expect("command object")
            .insert("target".to_string(), serde_json::json!("peer_only"));
        let error = decode_bridge_command(under_versioned)
            .expect_err("the target-bearing shape was introduced in V4");
        assert!(matches!(
            error,
            BridgeCommandDecodeError::UnsupportedProtocolVersion(_)
        ));

        let v4 = BridgeCommand::DeclareMemberOutboundTaint(BridgeOutboundTaintPayload {
            supervisor: sample_peer_spec(),
            epoch: 4,
            protocol_version: BridgeProtocolVersion::V4,
            target: Some(BridgeOutboundTaintTarget::PeerOnly),
            taint: None,
        });
        assert_eq!(
            decode_bridge_command(serde_json::to_value(&v4).expect("serialize V4 taint"))
                .expect("target-bearing V4 payload decodes"),
            v4
        );
    }

    #[test]
    fn bridge_outbound_taint_payload_rejects_unknown_fields() {
        let mut value = serde_json::to_value(BridgeOutboundTaintPayload {
            supervisor: sample_peer_spec(),
            epoch: 4,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            target: Some(BridgeOutboundTaintTarget::PeerOnly),
            taint: Some(meerkat_core::comms::SenderContentTaint::Tainted),
        })
        .expect("serialize payload");
        value
            .as_object_mut()
            .expect("payload object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        let result: Result<BridgeOutboundTaintPayload, _> = serde_json::from_value(value);
        assert!(
            result.is_err(),
            "unknown fields must fail loud at the wire boundary"
        );
    }

    #[test]
    fn bridge_command_deliver_member_input_with_injected_context_round_trip() {
        let cmd = BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
            objective_id: Some(meerkat_core::interaction::ObjectiveId::new()),
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: "input-1".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: vec![
                meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
            ],
            turn: None,
            outcome_tracking: None,
        });
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_delivery_interaction_tracking_is_absent_omitted_and_v4_gated() {
        let mut payload = BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: BridgeProtocolVersion::V4,
            input_id: "input-tracked".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: None,
        };
        let absent = serde_json::to_value(&payload).expect("serialize absent tracking marker");
        assert!(
            absent.get("outcome_tracking").is_none(),
            "absent marker must preserve the pre-field wire shape: {absent}"
        );

        payload.outcome_tracking = Some(BridgeOutcomeTracking::Interaction);
        let command = BridgeCommand::DeliverMemberInput(payload.clone());
        let value = serde_json::to_value(&command).expect("serialize tracked interaction");
        assert_eq!(value["outcome_tracking"], json!("interaction"));
        assert_eq!(
            decode_bridge_command(value).expect("V4 tracking marker decodes"),
            command
        );

        payload.protocol_version = BridgeProtocolVersion::V3;
        let error = decode_bridge_command(
            serde_json::to_value(BridgeCommand::DeliverMemberInput(payload))
                .expect("serialize under-versioned tracked interaction"),
        )
        .expect_err("the tracking marker belongs to the V4 semantic fold");
        assert!(matches!(
            error,
            BridgeCommandDecodeError::UnsupportedProtocolVersion(_)
        ));
    }

    /// Byte-compat pin: an empty `injected_context` is omitted from the
    /// serialized delivery payload, so deliveries without injected context
    /// stay byte-identical to the pre-field wire shape, and a pre-field
    /// payload (no key) deserializes to empty.
    #[test]
    fn bridge_delivery_payload_empty_injected_context_is_byte_compatible() {
        let payload = BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: "input-1".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: None,
        };
        let value = serde_json::to_value(&payload).expect("serialize payload");
        assert!(
            value.get("injected_context").is_none(),
            "empty injected_context must be omitted: {value}"
        );
        assert!(
            value.get("turn").is_none(),
            "absent turn directive must be omitted: {value}"
        );
        assert!(
            value.get("outcome_tracking").is_none(),
            "absent outcome tracking must be omitted: {value}"
        );
        assert!(
            value.get("transcript_interaction_id").is_none(),
            "absent transcript interaction id must preserve the legacy shape: {value}"
        );

        let mut pre_field = serde_json::to_value(&payload).expect("serialize payload");
        pre_field
            .as_object_mut()
            .expect("payload object")
            .remove("injected_context");
        let parsed: BridgeDeliveryPayload =
            serde_json::from_value(pre_field).expect("pre-field payload deserializes");
        assert!(parsed.injected_context.is_empty());
        assert!(parsed.transcript_interaction_id.is_none());
    }

    #[test]
    fn bridge_delivery_transcript_identity_is_absent_compatible_and_v4_gated() {
        let legacy = BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: BridgeProtocolVersion::V3,
            input_id: "input-legacy".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: None,
        });
        let mut value = serde_json::to_value(&legacy).expect("serialize legacy delivery");
        assert_eq!(
            decode_bridge_command(value.clone()).expect("omitted field remains decodable"),
            legacy
        );

        value["transcript_interaction_id"] = json!(uuid::Uuid::from_u128(0x1234).to_string());
        assert!(matches!(
            decode_bridge_command(value)
                .expect_err("the transcript identity carrier was introduced in V4"),
            BridgeCommandDecodeError::UnsupportedProtocolVersion(_)
        ));
    }

    #[test]
    fn bridge_command_observe_member_round_trip() {
        let cmd = BridgeCommand::ObserveMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_interrupt_member_round_trip() {
        let cmd = BridgeCommand::InterruptMember(sample_interrupt_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_hard_cancel_member_round_trip() {
        let cmd = BridgeCommand::HardCancelMember(sample_hard_cancel_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_tracked_input_cancel_round_trip() {
        let cmd = BridgeCommand::CancelTrackedMemberInput(sample_tracked_input_cancel_payload());
        assert_command_round_trip(&cmd);
        assert_eq!(cmd.protocol_version(), BridgeProtocolVersion::V4);

        let reply = BridgeReply::TrackedInputCancelled(BridgeTrackedInputCancelResponse {
            expected_member: sample_member_incarnation(),
            input_id: sample_tracked_input_cancel_payload().input_id,
            outcome: BridgeTrackedInputCancelOutcome::NoEffect,
        });
        let encoded = serde_json::to_value(&reply).expect("serialize tracked cancel reply");
        assert_eq!(encoded["result"], json!("tracked_input_cancelled"));
        let decoded: BridgeReply =
            serde_json::from_value(encoded).expect("decode tracked cancel reply");
        assert_eq!(decoded, reply);
    }

    #[test]
    fn bridge_command_hard_cancel_requires_operation_and_expected_run_truth() {
        let command = BridgeCommand::HardCancelMember(sample_hard_cancel_payload());
        let value = serde_json::to_value(&command).expect("serialize hard-cancel command");
        assert_eq!(
            value.get("operation_id"),
            Some(&json!(uuid::Uuid::from_u128(0xCA11).to_string())),
            "hard cancel must expose one stable operation id on the wire"
        );
        assert_eq!(
            value.get("expected_run_id"),
            Some(&json!(uuid::Uuid::from_u128(0xA11CE).to_string())),
            "hard cancel must expose the exact run fence on the wire"
        );

        for required in ["operation_id", "expected_run_id"] {
            let mut missing = value.clone();
            missing
                .as_object_mut()
                .expect("hard-cancel command object")
                .remove(required);
            let error = decode_bridge_command(missing)
                .expect_err("missing hard-cancel identity truth must fail closed");
            assert!(
                error.to_string().contains("missing field"),
                "missing {required} should be a strict decode failure: {error}"
            );
        }
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
        match &decoded {
            BridgeReply::Rejected { cause, reason } => {
                assert_eq!(*cause, BridgeRejectionCause::StaleSupervisor);
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
            SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION
        );
        assert_eq!(
            supervisor_bridge_supported_protocol_versions(),
            &[
                BridgeProtocolVersion::V2,
                BridgeProtocolVersion::V3,
                BridgeProtocolVersion::V4
            ]
        );
        // V4 is both current and the default for newly minted authorities: it
        // implements the multi-host families and durable supervisor rotation.
        // V2/V3 remain explicitly decodable for negotiated legacy peers.
        assert_eq!(
            supervisor_bridge_current_protocol_version(),
            BridgeProtocolVersion::V4
        );
        assert_eq!(
            supervisor_bridge_default_protocol_version(),
            BridgeProtocolVersion::V4
        );
        assert!(supervisor_bridge_protocol_version_supported(
            BridgeProtocolVersion::V2
        ));
        assert!(supervisor_bridge_protocol_version_supported(
            BridgeProtocolVersion::V3
        ));
        assert!(supervisor_bridge_protocol_version_supported(
            SUPERVISOR_BRIDGE_PROTOCOL_VERSION
        ));
        assert!(BridgeProtocolVersion::V4.supports_multi_host());
        assert!(!BridgeProtocolVersion::V3.supports_multi_host());
        assert!(BridgeProtocolVersion::from_supported_u32(1).is_err());
        assert!(BridgeProtocolVersion::from_supported_u32(5).is_err());
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
            SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION
        );
        assert_eq!(
            capabilities.supported_protocol_versions,
            vec![
                BridgeProtocolVersion::V2,
                BridgeProtocolVersion::V3,
                BridgeProtocolVersion::V4
            ]
        );
        // V4 host-capability facts default to the incapable/unreported
        // state — capability presence is always an explicit declaration.
        assert!(!capabilities.durable_sessions);
        assert!(!capabilities.autonomous_members);
        assert!(!capabilities.memory_store);
        assert!(!capabilities.mcp);
        assert!(!capabilities.approval_forwarding);
        assert!(capabilities.engine_version.is_empty());
        assert!(capabilities.resolvable_providers.is_empty());
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
            SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION
        );
        assert_eq!(
            capabilities.supported_protocol_versions,
            vec![
                BridgeProtocolVersion::V2,
                BridgeProtocolVersion::V3,
                BridgeProtocolVersion::V4
            ]
        );
        assert!(capabilities.deliver_member_input);
        assert!(capabilities.observe_member);
        assert!(capabilities.interrupt_member);
        assert!(!capabilities.hard_cancel_member);
        assert!(capabilities.retire_member);
        assert!(capabilities.destroy_member);
        assert!(capabilities.wire_member);
        assert!(capabilities.unwire_member);
        // V2-era payload carries none of the V4 capability fields — they
        // must decode to the incapable/unreported defaults (byte-compat).
        assert!(!capabilities.durable_sessions);
        assert!(!capabilities.autonomous_members);
        assert!(!capabilities.memory_store);
        assert!(!capabilities.mcp);
        assert!(!capabilities.approval_forwarding);
        assert!(capabilities.engine_version.is_empty());
        assert!(capabilities.resolvable_providers.is_empty());
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
    // The mob side maps these typed causes onto the MobMachine
    // `ClassifyBridgeRejectionRecovery` input, which owns the recoverable-vs-
    // fatal recovery verdict. Any accidental rename or new variant that skipped
    // the snake_case convention would silently change recovery behavior, so pin
    // the full matrix.

    #[test]
    fn bridge_rejection_cause_snake_case_round_trip_all_unit_variants() {
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
            // V4 unit causes.
            (BridgeRejectionCause::StaleFence, "stale_fence"),
            (BridgeRejectionCause::Unavailable, "unavailable"),
            (
                BridgeRejectionCause::SpecDigestMismatch,
                "spec_digest_mismatch",
            ),
            (
                BridgeRejectionCause::RealmBackendUnavailable,
                "realm_backend_unavailable",
            ),
            (
                BridgeRejectionCause::LiveTransportUnavailable,
                "live_transport_unavailable",
            ),
            (
                BridgeRejectionCause::LiveChannelAlreadyBound,
                "live_channel_already_bound",
            ),
            (
                BridgeRejectionCause::LiveChannelNotFound,
                "live_channel_not_found",
            ),
            (
                BridgeRejectionCause::ResumeSessionNotFound,
                "resume_session_not_found",
            ),
            (
                BridgeRejectionCause::LaunchModeUnsupported,
                "launch_mode_unsupported",
            ),
            (
                BridgeRejectionCause::LaunchModePlacementMismatch,
                "launch_mode_placement_mismatch",
            ),
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
            assert_eq!(&decoded, cause);
        }
    }

    #[test]
    fn bridge_rejection_cause_data_variants_round_trip_with_typed_detail() {
        use crate::wire::mob::WireControlScope;

        let cases: Vec<(BridgeRejectionCause, serde_json::Value)> = vec![
            (
                BridgeRejectionCause::StaleCursor {
                    watermark: 42,
                    generation: 3,
                },
                json!({"stale_cursor": {"watermark": 42, "generation": 3}}),
            ),
            (
                BridgeRejectionCause::OversizedEvent {
                    generation: 3,
                    durable_seq: 43,
                    next_seq: 44,
                    encoded_bytes: 600_000,
                    max_bytes: 524_288,
                },
                json!({"oversized_event": {
                    "generation": 3,
                    "durable_seq": 43,
                    "next_seq": 44,
                    "encoded_bytes": 600_000,
                    "max_bytes": 524_288,
                }}),
            ),
            (
                BridgeRejectionCause::HistoryRowTooLarge {
                    index: 9,
                    encoded_bytes: 600_000,
                    max_bytes: 524_288,
                },
                json!({"history_row_too_large": {
                    "index": 9,
                    "encoded_bytes": 600_000,
                    "max_bytes": 524_288,
                }}),
            ),
            (
                BridgeRejectionCause::SessionOwnershipConflict {
                    session_id: "session-1".to_string(),
                },
                json!({"session_ownership_conflict": {
                    "session_id": "session-1",
                }}),
            ),
            (
                BridgeRejectionCause::ScopeDenied {
                    required: WireControlScope::Live,
                    presented: vec![WireControlScope::List, WireControlScope::ReadHistory],
                },
                json!({"scope_denied": {
                    "required": "live",
                    "presented": ["list", "read_history"],
                }}),
            ),
            (
                BridgeRejectionCause::MaterializeBuildRejected {
                    cause: MemberBuildRejection::UnknownProviderForModel {
                        model: "mystery-model".to_string(),
                    },
                },
                json!({"materialize_build_rejected": {
                    "cause": {"unknown_provider_for_model": {"model": "mystery-model"}},
                }}),
            ),
            (
                BridgeRejectionCause::MaterializeBuildRejected {
                    cause: MemberBuildRejection::BindingUnresolvable {
                        kind: ConnectionTargetErrorKind::UnknownRealm,
                    },
                },
                json!({"materialize_build_rejected": {
                    "cause": {"binding_unresolvable": {"kind": "unknown_realm"}},
                }}),
            ),
            (
                BridgeRejectionCause::ModelUnresolvable {
                    model: "m1".to_string(),
                },
                json!({"model_unresolvable": {"model": "m1"}}),
            ),
            (
                BridgeRejectionCause::AuthBindingUnresolvable {
                    realm: "dev".to_string(),
                    binding: "default_anthropic".to_string(),
                },
                json!({"auth_binding_unresolvable": {"realm": "dev", "binding": "default_anthropic"}}),
            ),
            (
                BridgeRejectionCause::McpCommandMissing {
                    server: "docs".to_string(),
                },
                json!({"mcp_command_missing": {"server": "docs"}}),
            ),
            (
                BridgeRejectionCause::EnvKeyMissing {
                    key: "DOCS_TOKEN".to_string(),
                },
                json!({"env_key_missing": {"key": "DOCS_TOKEN"}}),
            ),
            (
                BridgeRejectionCause::HostEngineVersionChanged {
                    bound: "0.7.22".to_string(),
                    reported: "0.8.0".to_string(),
                },
                json!({"host_engine_version_changed": {"bound": "0.7.22", "reported": "0.8.0"}}),
            ),
            (
                BridgeRejectionCause::ModelNotRealtime {
                    model: "m1".to_string(),
                    provider: "anthropic".to_string(),
                },
                json!({"model_not_realtime": {"model": "m1", "provider": "anthropic"}}),
            ),
            (
                BridgeRejectionCause::LiveAdapterUnavailable {
                    provider: "gemini".to_string(),
                },
                json!({"live_adapter_unavailable": {"provider": "gemini"}}),
            ),
            (
                BridgeRejectionCause::LiveTransportUnsupported {
                    requested: "webrtc".to_string(),
                },
                json!({"live_transport_unsupported": {"requested": "webrtc"}}),
            ),
            (
                BridgeRejectionCause::CapabilityMissing {
                    capability: "durable_sessions".to_string(),
                },
                json!({"capability_missing": {"capability": "durable_sessions"}}),
            ),
        ];
        for (cause, expected) in cases {
            let value = serde_json::to_value(&cause).expect("serialize cause");
            assert_eq!(value, expected, "cause {cause:?} wire shape");
            let decoded: BridgeRejectionCause =
                serde_json::from_value(value).expect("decode cause");
            assert_eq!(decoded, cause);
        }
    }

    #[test]
    fn connection_target_error_kind_round_trips_snake_case() {
        let cases: &[(ConnectionTargetErrorKind, &str)] = &[
            (ConnectionTargetErrorKind::MissingRealm, "missing_realm"),
            (ConnectionTargetErrorKind::UnknownRealm, "unknown_realm"),
            (
                ConnectionTargetErrorKind::MissingDefaultBinding,
                "missing_default_binding",
            ),
            (
                ConnectionTargetErrorKind::InvalidRealmId,
                "invalid_realm_id",
            ),
            (
                ConnectionTargetErrorKind::InvalidBindingId,
                "invalid_binding_id",
            ),
            (
                ConnectionTargetErrorKind::RealmConfigInvalid,
                "realm_config_invalid",
            ),
            (ConnectionTargetErrorKind::BindingInvalid, "binding_invalid"),
            (
                ConnectionTargetErrorKind::ProviderMismatch,
                "provider_mismatch",
            ),
            (ConnectionTargetErrorKind::RealmChain, "realm_chain"),
        ];
        for (kind, expected) in cases {
            let value = serde_json::to_value(kind).expect("serialize kind");
            assert_eq!(value, json!(expected));
            let decoded: ConnectionTargetErrorKind =
                serde_json::from_value(value).expect("decode kind");
            assert_eq!(decoded, *kind);
        }
    }

    #[test]
    fn member_build_rejection_round_trips_every_variant() {
        let cases: Vec<(MemberBuildRejection, serde_json::Value)> = vec![
            (
                MemberBuildRejection::UnknownProviderForModel {
                    model: "m1".to_string(),
                },
                json!({"unknown_provider_for_model": {"model": "m1"}}),
            ),
            (
                MemberBuildRejection::BindingUnresolvable {
                    kind: ConnectionTargetErrorKind::MissingDefaultBinding,
                },
                json!({"binding_unresolvable": {"kind": "missing_default_binding"}}),
            ),
            (
                MemberBuildRejection::ProviderAuth {
                    kind: meerkat_core::AuthErrorKind::InteractiveLoginRequired,
                },
                json!({"provider_auth": {"kind": "interactive_login_required"}}),
            ),
            (
                MemberBuildRejection::SelfHostedServerMissing {
                    server_id: "llama-box".to_string(),
                },
                json!({"self_hosted_server_missing": {"server_id": "llama-box"}}),
            ),
        ];
        for (rejection, expected) in cases {
            let value = serde_json::to_value(&rejection).expect("serialize rejection");
            assert_eq!(value, expected, "rejection {rejection:?} wire shape");
            let decoded: MemberBuildRejection =
                serde_json::from_value(value).expect("decode rejection");
            assert_eq!(decoded, rejection);
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
                    "default_protocol_version": SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION,
                    "supported_protocol_versions": [
                        BridgeProtocolVersion::V2,
                        BridgeProtocolVersion::V3,
                        BridgeProtocolVersion::V4,
                    ],
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

        assert_reply_round_trip(
            BridgeReply::Delivery(BridgeDeliveryResponse {
                input_id: "in-3".to_string(),
                canonical_input_id: None,
                outcome: BridgeDeliveryOutcome::Rejected {
                    cause: BridgeDeliveryRejectionCause::OutcomeJournalFull {
                        retained: 256,
                        limit: 256,
                    },
                    reason: "consume and acknowledge outcomes".to_string(),
                },
            }),
            json!({
                "result": "delivery",
                "input_id": "in-3",
                "canonical_input_id": null,
                "outcome": {
                    "outcome": "rejected",
                    "cause": {
                        "kind": "outcome_journal_full",
                        "retained": 256,
                        "limit": 256,
                    },
                    "reason": "consume and acknowledge outcomes",
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

    // -----------------------------------------------------------------------
    // V4 multi-host protocol — command/reply round-trips, version gating,
    // launch-mode closure, operator vocabulary, turn directive compat.
    // -----------------------------------------------------------------------

    fn v4() -> BridgeProtocolVersion {
        BridgeProtocolVersion::V4
    }

    fn sample_spawn_spec() -> MemberOperatorSpawnSpec {
        MemberOperatorSpawnSpec {
            profile: "worker".to_string(),
            member_id: "worker-2".to_string(),
            initial_message: Some(meerkat_core::types::ContentInput::Text("go".to_string())),
            runtime_mode: Some(WireMobRuntimeMode::TurnDriven),
            launch_mode: Some(WireMemberLaunchMode::Fresh),
            auto_wire_parent: Some(true),
            placement: Some("host-b-peer-id".to_string()),
            requested_tool_access_policy_present: true,
            resolved_tool_access_policy: Some(WireResolvedToolAccessPolicy::AllowList(vec![
                "member_status".to_string(),
            ])),
        }
    }

    fn all_v4_commands() -> Vec<BridgeCommand> {
        vec![
            BridgeCommand::HardCancelMember(sample_hard_cancel_payload()),
            BridgeCommand::CancelTrackedMemberInput(sample_tracked_input_cancel_payload()),
            BridgeCommand::ReadMemberHistory(BridgeReadHistoryPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                from_index: Some(10),
                limit: Some(50),
            }),
            BridgeCommand::PollMemberEvents(BridgePollEventsPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                cursor: BridgeEventCursor::At {
                    generation: 2,
                    seq: 77,
                },
                max: Some(100),
                outcome_acks: vec![BridgeTurnOutcomeAck {
                    generation: 2,
                    fence_token: 9,
                    input_id: "input-previous".to_string(),
                }],
                max_outcomes: Some(8),
                wait_ms: Some(2_000),
            }),
            BridgeCommand::OpenMemberLiveChannel(BridgeLiveOpenPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                turning_mode: Some(RealtimeTurningMode::ExplicitCommit),
                transport: Some(LiveOpenTransport::Websocket),
            }),
            BridgeCommand::CloseMemberLiveChannel(BridgeLiveChannelPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                channel_id: "chan-1".to_string(),
            }),
            BridgeCommand::MemberLiveChannelStatus(BridgeLiveStatusPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                channel_id: Some("chan-1".to_string()),
            }),
            // ADJ-P6B-2: channel-less status addresses "the member's active
            // channel" — the reply-loss discovery shape.
            BridgeCommand::MemberLiveChannelStatus(BridgeLiveStatusPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                channel_id: None,
            }),
            BridgeCommand::ControlMemberLiveChannel(BridgeLiveControlPayload {
                supervisor: sample_peer_spec(),
                epoch: 9,
                protocol_version: v4(),
                expected_member: sample_member_incarnation(),
                channel_id: "chan-1".to_string(),
                verb: BridgeLiveControlVerb::Truncate {
                    item_id: "item-3".to_string(),
                    content_index: 0,
                    audio_played_ms: 1_500,
                },
            }),
            BridgeCommand::BindHost(BridgeHostBindPayload {
                supervisor: sample_peer_spec(),
                epoch: 1,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
                expected_host_peer_id: "host-peer".to_string(),
                expected_address: "tcp://10.0.0.2:7100".to_string(),
                bootstrap_proof: BridgeHostBootstrapProof::new("host-bootstrap-proof"),
                required_capabilities: Default::default(),
            }),
            BridgeCommand::RebindHost(BridgeHostRebindPayload {
                supervisor: sample_peer_spec(),
                epoch: 2,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
                required_capabilities: Default::default(),
            }),
            BridgeCommand::RevokeHost(BridgeHostRevokePayload {
                supervisor: sample_peer_spec(),
                epoch: 2,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
            }),
            BridgeCommand::MaterializeMember(Box::new(BridgeMaterializePayload {
                supervisor: sample_peer_spec(),
                epoch: 1,
                binding_generation: 1,
                protocol_version: v4(),
                generation: 1,
                fence_token: 3,
                spec: crate::wire::portable_spec::sample_portable_member_spec(),
                spec_digest: "d".repeat(64),
                launch: MaterializeLaunchMode::Resume {
                    session_id: "sess-9".to_string(),
                },
            })),
            BridgeCommand::ReleaseMember(BridgeReleasePayload {
                supervisor: sample_peer_spec(),
                epoch: 1,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
                agent_identity: "worker-1".to_string(),
                generation: 1,
                fence_token: 3,
            }),
            BridgeCommand::InstallPeerTrust(BridgePeerTrustPayload {
                supervisor: sample_peer_spec(),
                epoch: 1,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
                agent_identity: "worker-1".to_string(),
                peer: sample_peer_spec(),
            }),
            BridgeCommand::RemovePeerTrust(BridgePeerTrustPayload {
                supervisor: sample_peer_spec(),
                epoch: 1,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
                agent_identity: "worker-1".to_string(),
                peer: sample_peer_spec(),
            }),
            BridgeCommand::HostStatus(BridgeHostStatusPayload {
                supervisor: sample_peer_spec(),
                epoch: 1,
                binding_generation: 1,
                protocol_version: v4(),
                mob_id: "mob-1".to_string(),
            }),
            BridgeCommand::MemberOperatorRequest(BridgeMemberOperatorPayload {
                agent_identity: "worker-1".to_string(),
                requester_generation: 2,
                requester_fence_token: 9,
                requester_host_id: "host-a".to_string(),
                requester_host_binding_generation: 4,
                requester_member_session_id: "member-session-a".to_string(),
                request_id: "req-1".to_string(),
                op: MemberOperatorOp::SpawnMember(Box::new(sample_spawn_spec())),
                protocol_version: v4(),
            }),
        ]
    }

    #[test]
    fn v4_commands_round_trip_and_decode_through_versioned_decoder() {
        for cmd in all_v4_commands() {
            assert_command_round_trip(&cmd);
            assert_eq!(cmd.protocol_version(), BridgeProtocolVersion::V4);
            let value = serde_json::to_value(&cmd).expect("serialize command");
            decode_bridge_command(value).expect("V4 command must decode on a V4 binary");
        }
    }

    #[test]
    fn v4_command_with_future_protocol_version_rejects_before_serde() {
        for cmd in all_v4_commands() {
            let mut value = serde_json::to_value(&cmd).expect("serialize command");
            value
                .as_object_mut()
                .expect("command object")
                .insert("protocol_version".to_string(), json!(5));
            let err = decode_bridge_command(value)
                .expect_err("future protocol version must reject pre-serde");
            assert!(
                matches!(err, BridgeCommandDecodeError::UnsupportedProtocolVersion(_)),
                "expected typed UnsupportedProtocolVersion, got {err:?}"
            );
        }
    }

    #[test]
    fn every_v4_only_command_rejects_v3_with_typed_version_error() {
        let mut commands = all_v4_commands();
        commands.push(BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: BridgeProtocolVersion::V4,
            input_id: "placed-v4".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("placed".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: Some(BridgeMemberIncarnation {
                mob_id: "mob-1".to_string(),
                agent_identity: "worker-1".to_string(),
                host_id: "host-1".to_string(),
                binding_generation: 1,
                member_session_id: "session-1".to_string(),
                generation: 3,
                fence_token: 7,
            }),
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: None,
        }));
        commands.push(BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: BridgeProtocolVersion::V4,
            input_id: "turn-v4".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("turn".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: Some(BridgeTurnDirective {
                correlation: BridgeTurnCorrelation {
                    run_id: "run-1".to_string(),
                    step_id: "step-1".to_string(),
                },
                tool_overlay: None,
            }),
            outcome_tracking: None,
        }));

        for command in commands {
            let mut value = serde_json::to_value(&command).expect("serialize V4 command");
            value["protocol_version"] = json!(3);
            let error = decode_bridge_command(value)
                .expect_err("a V4-only command stamped V3 must reject before serving");
            assert!(
                matches!(
                    error,
                    BridgeCommandDecodeError::UnsupportedProtocolVersion(_)
                ),
                "expected typed version rejection for {command:?}, got {error:?}"
            );
            assert!(error.to_string().contains("minimum 4"), "{error}");
        }
    }

    #[test]
    fn old_v3_hard_cancel_shape_rejects_by_version_before_new_fields() {
        let fixture = json!({
            "command": "hard_cancel_member",
            "supervisor": sample_peer_spec(),
            "epoch": 4,
            "protocol_version": 3,
            "reason": "legacy immediate cancel"
        });
        let error = decode_bridge_command(fixture)
            .expect_err("old V3 hard cancel must not decode as the V4 exact-run shape");
        assert!(
            matches!(
                error,
                BridgeCommandDecodeError::UnsupportedProtocolVersion(_)
            ),
            "expected typed version rejection, got {error:?}"
        );
        assert!(error.to_string().contains("HardCancelMember"));
    }

    #[test]
    fn v3_capabilities_fixture_decodes_and_default_extensions_omit() {
        let fixture = json!({
            "current_protocol_version": 3,
            "default_protocol_version": 3,
            "supported_protocol_versions": [2, 3],
            "deliver_member_input": true,
            "observe_member": true,
            "interrupt_member": true,
            "hard_cancel_member": false,
            "retire_member": true,
            "destroy_member": true,
            "wire_member": true,
            "unwire_member": true
        });
        let capabilities: BridgeCapabilities =
            serde_json::from_value(fixture).expect("origin-V3 capabilities must decode");
        assert!(!capabilities.durable_sessions);
        assert!(capabilities.engine_version.is_empty());

        let encoded = serde_json::to_value(BridgeCapabilities {
            current_protocol_version: BridgeProtocolVersion::V3,
            default_protocol_version: BridgeProtocolVersion::V3,
            supported_protocol_versions: vec![BridgeProtocolVersion::V2, BridgeProtocolVersion::V3],
            ..BridgeCapabilities::default()
        })
        .expect("serialize V3 projection");
        for field in [
            "durable_sessions",
            "autonomous_members",
            "tracked_input_cancel",
            "memory_store",
            "mcp",
            "engine_version",
            "approval_forwarding",
            "resolvable_providers",
        ] {
            assert!(encoded.get(field).is_none(), "V4 field leaked: {field}");
        }
    }

    /// FLAG-1 pin: every host-addressed payload carries an explicit,
    /// REQUIRED `mob_id`. Name-derived mob identity (parsing
    /// `supervisor.name`) is forbidden, so a payload without the field must
    /// fail decode instead of falling back to folklore.
    #[test]
    fn host_addressed_payloads_require_explicit_mob_id() {
        let host_addressed = [
            "bind_host",
            "rebind_host",
            "revoke_host",
            "release_member",
            "install_peer_trust",
            "remove_peer_trust",
            "host_status",
        ];
        let mut seen = Vec::new();
        for cmd in all_v4_commands() {
            let mut value = serde_json::to_value(&cmd).expect("serialize command");
            let tag = value["command"]
                .as_str()
                .expect("command tag present")
                .to_string();
            if !host_addressed.contains(&tag.as_str()) {
                continue;
            }
            seen.push(tag.clone());
            assert_eq!(
                value["mob_id"],
                json!("mob-1"),
                "{tag} must carry the explicit mob_id"
            );
            value
                .as_object_mut()
                .expect("command object")
                .remove("mob_id");
            assert!(
                decode_bridge_command(value).is_err(),
                "{tag} without mob_id must fail decode (never name-derived identity)"
            );
        }
        for tag in host_addressed {
            assert!(
                seen.contains(&tag.to_string()),
                "fixture inventory must cover {tag}"
            );
        }
    }

    /// ADJ-1 absence pin: `budget_seed` was deleted from
    /// `BridgeMaterializePayload` — the digest-covered
    /// `spec.overlay.budget_limits` is the single budget carrier (§18 O4).
    /// A payload still carrying the deleted sibling FAILS decode (the
    /// `budget_split_policy` absence-pin pattern).
    #[test]
    fn materialize_payload_rejects_deleted_budget_seed() {
        let cmd = BridgeCommand::MaterializeMember(Box::new(BridgeMaterializePayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            binding_generation: 1,
            protocol_version: v4(),
            generation: 1,
            fence_token: 3,
            spec: crate::wire::portable_spec::sample_portable_member_spec(),
            spec_digest: "d".repeat(64),
            launch: MaterializeLaunchMode::Fresh {},
        }));
        let mut value = serde_json::to_value(&cmd).expect("serialize command");
        value
            .as_object_mut()
            .expect("command object")
            .insert("budget_seed".to_string(), json!({ "max_tokens": 10_000 }));
        let err = decode_bridge_command(value)
            .expect_err("deleted budget_seed must fail closed at the wire boundary");
        assert!(
            err.to_string().contains("unknown field `budget_seed`"),
            "unexpected error: {err}"
        );
    }

    /// Member identity has one carrier: the digest-covered portable spec.
    /// A payload-level sibling fails closed before serving, so it can never
    /// diverge from the identity that the host actually builds.
    #[test]
    fn materialize_payload_rejects_duplicate_agent_identity_carrier() {
        let cmd = BridgeCommand::MaterializeMember(Box::new(BridgeMaterializePayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            binding_generation: 1,
            protocol_version: v4(),
            generation: 1,
            fence_token: 3,
            spec: crate::wire::portable_spec::sample_portable_member_spec(),
            spec_digest: "d".repeat(64),
            launch: MaterializeLaunchMode::Fresh {},
        }));
        let mut value = serde_json::to_value(&cmd).expect("serialize command");
        value
            .as_object_mut()
            .expect("command object")
            .insert("agent_identity".to_string(), json!("different-worker"));
        let err = decode_bridge_command(value)
            .expect_err("duplicate agent_identity carrier must fail closed");
        assert!(
            err.to_string().contains("unknown field `agent_identity`"),
            "unexpected error: {err}"
        );
    }

    /// FLAG-2 pin: `RebindHost` / `HostRebound` wire tags and fail-closed
    /// unknown-field posture.
    #[test]
    fn rebind_host_round_trips_and_fails_closed() {
        let cmd = BridgeCommand::RebindHost(BridgeHostRebindPayload {
            supervisor: sample_peer_spec(),
            epoch: 5,
            binding_generation: 1,
            protocol_version: v4(),
            mob_id: "mob-1".to_string(),
            required_capabilities: Default::default(),
        });
        assert_command_round_trip(&cmd);
        let mut value = serde_json::to_value(&cmd).expect("serialize command");
        assert_eq!(value["command"], json!("rebind_host"));
        value
            .as_object_mut()
            .expect("command object")
            .insert("smuggled".to_string(), json!(true));
        assert!(
            decode_bridge_command(value).is_err(),
            "unknown RebindHost fields must fail closed"
        );

        let reply = BridgeReply::HostRebound(BridgeHostReboundResponse {
            host_peer_id: "host-peer".to_string(),
            binding_generation: 1,
            capabilities: BridgeCapabilities::default(),
            live_endpoint: None,
        });
        let value = serde_json::to_value(&reply).expect("serialize reply");
        assert_eq!(value["result"], json!("host_rebound"));
        assert!(
            value.get("live_endpoint").is_none(),
            "absent live endpoint must be omitted (absence = live-incapable, no shadow)"
        );
        assert_reply_value_round_trip(&reply);
    }

    #[test]
    fn revoke_host_round_trips_and_requires_exact_closed_fields() {
        let cmd = BridgeCommand::RevokeHost(BridgeHostRevokePayload {
            supervisor: sample_peer_spec(),
            epoch: 5,
            binding_generation: 1,
            protocol_version: v4(),
            mob_id: "mob-1".to_string(),
        });
        assert_command_round_trip(&cmd);
        let mut value = serde_json::to_value(&cmd).expect("serialize command");
        assert_eq!(value["command"], json!("revoke_host"));
        value
            .as_object_mut()
            .expect("command object")
            .insert("smuggled".to_string(), json!(true));
        assert!(
            decode_bridge_command(value).is_err(),
            "unknown RevokeHost fields must fail closed"
        );

        let reply = BridgeReply::HostRevoked(BridgeHostRevokedResponse {
            host_peer_id: "host-peer".to_string(),
            mob_id: "mob-1".to_string(),
            epoch: 5,
            binding_generation: 1,
            released_members: vec!["worker-1".to_string()],
        });
        let value = serde_json::to_value(&reply).expect("serialize reply");
        assert_eq!(value["result"], json!("host_revoked"));
        assert_reply_value_round_trip(&reply);
    }

    /// Byte-compat pin: a V3-era DeliverMemberInput fixture (no `turn`
    /// key, raw JSON as an old sender emits it) still decodes on a V4
    /// receiver.
    #[test]
    fn v3_delivery_fixture_without_turn_still_decodes() {
        let fixture = json!({
            "command": "deliver_member_input",
            "supervisor": {
                "name": "mob/__mob_supervisor__",
                "peer_id": "00000000-0000-0000-0000-00000000bbbb",
                "address": "inproc://mob/__mob_supervisor__",
            },
            "epoch": 7,
            "protocol_version": 3,
            "input_id": "input-legacy",
            "content": "hello from a V3 sender",
            "handling_mode": "queue",
        });
        match decode_bridge_command(fixture).expect("V3 delivery fixture decodes") {
            BridgeCommand::DeliverMemberInput(payload) => {
                assert_eq!(payload.protocol_version, BridgeProtocolVersion::V3);
                assert!(payload.turn.is_none(), "absent turn decodes as None");
                assert!(
                    payload.outcome_tracking.is_none(),
                    "absent tracking marker decodes as None"
                );
                assert!(payload.injected_context.is_empty());
            }
            other => panic!("expected DeliverMemberInput, got {other:?}"),
        }
    }

    #[test]
    fn delivery_with_turn_directive_round_trips_and_serializes_turn_key() {
        let payload = BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: v4(),
            input_id: "input-directed".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("step".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: Some(BridgeTurnDirective {
                correlation: BridgeTurnCorrelation {
                    run_id: "run-1".to_string(),
                    step_id: "draft".to_string(),
                },
                tool_overlay: Some(meerkat_core::service::PublicTurnToolOverlay {
                    allowed_tools: None,
                    blocked_tools: None,
                }),
            }),
            outcome_tracking: None,
        };
        let value = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(value["turn"]["correlation"]["run_id"], json!("run-1"));
        let decoded: BridgeDeliveryPayload = serde_json::from_value(value).expect("decode payload");
        assert_eq!(decoded, payload);
        assert_command_round_trip(&BridgeCommand::DeliverMemberInput(payload));
    }

    /// Simulated pre-V4 receiver: the old payload shape (no `turn` field,
    /// `deny_unknown_fields`) must fail closed on a turn-bearing delivery —
    /// never run it as an undirected turn.
    #[test]
    fn turn_bearing_delivery_fails_closed_on_pre_v4_receiver_shape() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct PreV4DeliveryPayload {
            supervisor: BridgePeerSpec,
            epoch: u64,
            protocol_version: BridgeProtocolVersion,
            input_id: String,
            content: meerkat_core::types::ContentInput,
            handling_mode: meerkat_core::types::HandlingMode,
            #[serde(default)]
            injected_context: Vec<meerkat_core::types::ContentInput>,
        }

        let mut value = serde_json::to_value(BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: BridgeProtocolVersion::V3,
            input_id: "input-directed".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("step".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: None,
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: None,
        })
        .expect("serialize payload");
        value["turn"] = json!({
            "correlation": { "run_id": "run-1", "step_id": "draft" },
        });

        let err = serde_json::from_value::<PreV4DeliveryPayload>(value)
            .expect_err("old receivers must reject a turn-bearing delivery");
        assert!(
            err.to_string().contains("turn") || err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    /// Simulated pre-marker V4 receiver: explicit interaction custody must
    /// fail loud at `deny_unknown_fields`, never degrade into an ordinary
    /// placed delivery that drops its completion handle.
    #[test]
    fn tracked_interaction_fails_closed_on_pre_marker_v4_receiver_shape() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct PreTrackingDeliveryPayload {
            supervisor: BridgePeerSpec,
            epoch: u64,
            protocol_version: BridgeProtocolVersion,
            input_id: String,
            content: meerkat_core::types::ContentInput,
            handling_mode: meerkat_core::types::HandlingMode,
            #[serde(default)]
            objective_id: Option<meerkat_core::interaction::ObjectiveId>,
            #[serde(default)]
            expected_member: Option<BridgeMemberIncarnation>,
            #[serde(default)]
            injected_context: Vec<meerkat_core::types::ContentInput>,
            #[serde(default)]
            turn: Option<BridgeTurnDirective>,
        }

        let value = serde_json::to_value(BridgeDeliveryPayload {
            objective_id: None,
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: BridgeProtocolVersion::V4,
            input_id: "input-tracked".to_string(),
            transcript_interaction_id: None,
            content: meerkat_core::types::ContentInput::Text("work".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            expected_member: Some(sample_member_incarnation()),
            injected_context: Vec::new(),
            turn: None,
            outcome_tracking: Some(BridgeOutcomeTracking::Interaction),
        })
        .expect("serialize tracked interaction");

        let error = serde_json::from_value::<PreTrackingDeliveryPayload>(value)
            .expect_err("pre-marker V4 receivers must reject explicit outcome custody");
        assert!(
            error.to_string().contains("outcome_tracking")
                || error.to_string().contains("unknown field"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn materialize_launch_mode_is_closed_fresh_resume_only() {
        let fresh: MaterializeLaunchMode =
            serde_json::from_value(json!({"mode": "fresh"})).expect("fresh decodes");
        assert_eq!(fresh, MaterializeLaunchMode::Fresh {});
        let resume: MaterializeLaunchMode =
            serde_json::from_value(json!({"mode": "resume", "session_id": "sess-1"}))
                .expect("resume decodes");
        assert_eq!(
            resume,
            MaterializeLaunchMode::Resume {
                session_id: "sess-1".to_string()
            }
        );

        assert!(
            serde_json::from_value::<MaterializeLaunchMode>(json!({"mode": "fork"})).is_err(),
            "fork must be unrepresentable on the materialize wire (A6/§19.L1)"
        );
        assert!(
            serde_json::from_value::<MaterializeLaunchMode>(json!({
                "mode": "fresh",
                "source_member_id": "smuggled",
            }))
            .is_err(),
            "unknown launch-mode fields must fail closed"
        );
    }

    #[test]
    fn materialize_launch_outcome_and_disposal_round_trip_snake_case() {
        for (outcome, expected) in [
            (MaterializeLaunchOutcome::Fresh, json!("fresh")),
            (MaterializeLaunchOutcome::ResumedLive, json!("resumed_live")),
            (
                MaterializeLaunchOutcome::ResumedFromSnapshot,
                json!("resumed_from_snapshot"),
            ),
        ] {
            let value = serde_json::to_value(outcome).expect("serialize outcome");
            assert_eq!(value, expected);
            let decoded: MaterializeLaunchOutcome =
                serde_json::from_value(value).expect("decode outcome");
            assert_eq!(decoded, outcome);
        }

        for (disposal, expected) in [
            (
                MemberSessionDisposal::Archived,
                json!({"disposal": "archived"}),
            ),
            (
                MemberSessionDisposal::AlreadyArchived,
                json!({"disposal": "already_archived"}),
            ),
            (
                MemberSessionDisposal::RuntimeReleasedOnly {
                    cause: RuntimeReleaseCause::NoDurableSessions,
                },
                json!({"disposal": "runtime_released_only", "cause": "no_durable_sessions"}),
            ),
            (
                MemberSessionDisposal::RuntimeReleasedOnly {
                    cause: RuntimeReleaseCause::HostOwnedSession,
                },
                json!({"disposal": "runtime_released_only", "cause": "host_owned_session"}),
            ),
        ] {
            let value = serde_json::to_value(disposal).expect("serialize disposal");
            assert_eq!(value, expected);
            let decoded: MemberSessionDisposal =
                serde_json::from_value(value).expect("decode disposal");
            assert_eq!(decoded, disposal);
        }
    }

    #[test]
    fn bridge_event_cursor_round_trips_and_rejects_unknown_fields() {
        let tail = serde_json::to_value(BridgeEventCursor::Tail).expect("serialize tail");
        assert_eq!(tail, json!({"cursor": "tail"}));
        let at = serde_json::to_value(BridgeEventCursor::At {
            generation: 2,
            seq: 77,
        })
        .expect("serialize at");
        assert_eq!(at, json!({"cursor": "at", "generation": 2, "seq": 77}));
        let decoded: BridgeEventCursor = serde_json::from_value(at).expect("decode at");
        assert_eq!(
            decoded,
            BridgeEventCursor::At {
                generation: 2,
                seq: 77
            }
        );
        assert!(
            serde_json::from_value::<BridgeEventCursor>(
                json!({"cursor": "at", "generation": 2, "seq": 7, "stream": "task"})
            )
            .is_err(),
            "unknown cursor fields must fail closed (seq domain is durable StoredEvent.seq only)"
        );
    }

    #[test]
    fn wire_flow_turn_outcome_round_trips_all_terminals() {
        let cases: Vec<(WireFlowTurnOutcome, serde_json::Value)> = vec![
            (WireFlowTurnOutcome::RunCompleted, json!("run_completed")),
            (
                WireFlowTurnOutcome::ExtractionSucceeded,
                json!("extraction_succeeded"),
            ),
            (
                WireFlowTurnOutcome::ExtractionFailed {
                    detail: WireFlowFailureDetail::complete("schema".to_string()),
                },
                json!({"extraction_failed": {"detail": {
                    "text": "schema",
                    "original_utf8_bytes": 6,
                    "truncated": false
                }}}),
            ),
            (
                WireFlowTurnOutcome::RunFailed {
                    detail: WireFlowFailureDetail::complete("boom".to_string()),
                },
                json!({"run_failed": {"detail": {
                    "text": "boom",
                    "original_utf8_bytes": 4,
                    "truncated": false
                }}}),
            ),
            (
                WireFlowTurnOutcome::InteractionComplete,
                json!("interaction_complete"),
            ),
            (
                WireFlowTurnOutcome::InteractionCallbackPending,
                json!("interaction_callback_pending"),
            ),
            (
                WireFlowTurnOutcome::InteractionFailed {
                    detail: WireFlowFailureDetail::complete("timeout".to_string()),
                },
                json!({"interaction_failed": {"detail": {
                    "text": "timeout",
                    "original_utf8_bytes": 7,
                    "truncated": false
                }}}),
            ),
            (WireFlowTurnOutcome::ChannelClosed, json!("channel_closed")),
        ];
        assert_eq!(
            cases.len(),
            8,
            "seven flow-turn terminals + channel-close (§18.1/§18.11)"
        );
        for (outcome, expected) in cases {
            let value = serde_json::to_value(&outcome).expect("serialize outcome");
            assert_eq!(value, expected, "outcome {outcome:?} wire shape");
            let decoded: WireFlowTurnOutcome =
                serde_json::from_value(value).expect("decode outcome");
            assert_eq!(decoded, outcome);
        }

        let cursor = MemberEventCursor {
            generation: 3,
            seq: 41,
        };
        let value = serde_json::to_value(cursor).expect("serialize member event cursor");
        assert_eq!(value, json!({"generation": 3, "seq": 41}));
        let decoded: MemberEventCursor = serde_json::from_value(value).expect("decode cursor");
        assert_eq!(decoded, cursor);
    }

    /// §15.10 pin: the operator vocabulary covers EXACTLY the twelve
    /// operator tools — the match below fails to compile if a variant is
    /// added or removed without updating this pin, and the fixtures prove
    /// each variant's wire tag.
    #[test]
    fn member_operator_op_covers_exactly_the_twelve_operator_tools() {
        let ops: Vec<(MemberOperatorOp, &str)> = vec![
            (
                MemberOperatorOp::SpawnMember(Box::new(sample_spawn_spec())),
                "spawn_member",
            ),
            (
                MemberOperatorOp::SpawnManyMembers {
                    specs: vec![sample_spawn_spec()],
                },
                "spawn_many_members",
            ),
            (
                MemberOperatorOp::RetireMember {
                    member_id: "w1".to_string(),
                },
                "retire_member",
            ),
            (
                MemberOperatorOp::ForceCancelMember {
                    member_id: "w1".to_string(),
                },
                "force_cancel_member",
            ),
            (
                MemberOperatorOp::MemberStatus {
                    member_id: "w1".to_string(),
                },
                "member_status",
            ),
            (
                MemberOperatorOp::WireMembers {
                    member_id: "w1".to_string(),
                    peer_member_id: "w2".to_string(),
                },
                "wire_members",
            ),
            (
                MemberOperatorOp::UnwireMembers {
                    member_id: "w1".to_string(),
                    peer_member_id: "w2".to_string(),
                },
                "unwire_members",
            ),
            (MemberOperatorOp::ListMembers, "list_members"),
            (MemberOperatorOp::MobListFlows, "mob_list_flows"),
            (
                MemberOperatorOp::MobRunFlow {
                    flow_id: "review".to_string(),
                    params: Some(WireOpaqueJson::from_value(&json!({"prompt": "go"}))),
                },
                "mob_run_flow",
            ),
            (
                MemberOperatorOp::MobFlowStatus {
                    run_id: "run-1".to_string(),
                },
                "mob_flow_status",
            ),
            (
                MemberOperatorOp::MobCancelFlow {
                    run_id: "run-1".to_string(),
                },
                "mob_cancel_flow",
            ),
        ];
        assert_eq!(ops.len(), 12, "exactly twelve operator ops (§15.10)");
        for (op, tag) in &ops {
            // Compile-time closure pin: adding a variant breaks this match.
            match op {
                MemberOperatorOp::SpawnMember(_)
                | MemberOperatorOp::SpawnManyMembers { .. }
                | MemberOperatorOp::RetireMember { .. }
                | MemberOperatorOp::ForceCancelMember { .. }
                | MemberOperatorOp::MemberStatus { .. }
                | MemberOperatorOp::WireMembers { .. }
                | MemberOperatorOp::UnwireMembers { .. }
                | MemberOperatorOp::ListMembers
                | MemberOperatorOp::MobListFlows
                | MemberOperatorOp::MobRunFlow { .. }
                | MemberOperatorOp::MobFlowStatus { .. }
                | MemberOperatorOp::MobCancelFlow { .. } => {}
            }
            let value = serde_json::to_value(op).expect("serialize op");
            assert_eq!(value["op"], json!(tag), "op tag pin for {tag}");
            let decoded: MemberOperatorOp = serde_json::from_value(value).expect("decode op");
            let reencoded = serde_json::to_value(&decoded).expect("reserialize op");
            assert_eq!(
                reencoded,
                serde_json::to_value(op).expect("serialize op"),
                "operator op round-trip must preserve wire shape"
            );
        }

        assert!(
            serde_json::from_value::<MemberOperatorOp>(json!({"op": "adopt_session"})).is_err(),
            "unknown operator ops must fail decode (closed vocabulary)"
        );
    }

    /// O3 pin: the spawn op carries the requested-policy fact and the
    /// resolved policy as SEPARATE wire keys, and the requested fact is
    /// required (absence is not launderable into "not requested").
    #[test]
    fn spawn_op_carries_two_separate_tool_policy_facts() {
        let value = serde_json::to_value(sample_spawn_spec()).expect("serialize spawn spec");
        assert_eq!(value["requested_tool_access_policy_present"], json!(true));
        assert_eq!(
            value["resolved_tool_access_policy"]["type"],
            json!("allow_list")
        );

        let mut without_requested = value.clone();
        without_requested
            .as_object_mut()
            .expect("spawn spec object")
            .remove("requested_tool_access_policy_present");
        assert!(
            serde_json::from_value::<MemberOperatorSpawnSpec>(without_requested).is_err(),
            "requested_tool_access_policy_present is a required fact (O3)"
        );

        let mut with_inherit = value;
        with_inherit["resolved_tool_access_policy"] = json!({"type": "inherit"});
        assert!(
            serde_json::from_value::<MemberOperatorSpawnSpec>(with_inherit).is_err(),
            "inherit is unrepresentable in the resolved policy (O3)"
        );
    }

    fn assert_reply_value_round_trip(reply: &BridgeReply) {
        let value = serde_json::to_value(reply).expect("serialize reply");
        let decoded: BridgeReply = serde_json::from_value(value.clone()).expect("decode reply");
        let reencoded = serde_json::to_value(&decoded).expect("reserialize reply");
        assert_eq!(
            value, reencoded,
            "reply round-trip must preserve wire shape"
        );
    }

    #[test]
    fn stale_member_residency_omits_absent_current_without_fabrication() {
        let expected = sample_member_incarnation();
        let cause = BridgeDeliveryRejectionCause::StaleMemberResidency {
            expected: expected.clone(),
            current: None,
        };
        let value = serde_json::to_value(&cause).expect("serialize stale residency cause");
        assert_eq!(value["kind"], json!("stale_member_residency"));
        assert_eq!(value["expected"]["host_id"], json!(expected.host_id));
        assert_eq!(
            value["expected"]["binding_generation"],
            json!(expected.binding_generation)
        );
        assert!(
            value.get("current").is_none(),
            "an absent replacement is omitted instead of fabricated or encoded as an SDK-required null"
        );
        let decoded: BridgeDeliveryRejectionCause =
            serde_json::from_value(value).expect("decode stale residency cause");
        assert_eq!(decoded, cause);
        assert!(cause.to_string().contains("current=None"));
    }

    #[test]
    fn v4_replies_round_trip() {
        let capabilities = BridgeCapabilities {
            durable_sessions: true,
            autonomous_members: true,
            tracked_input_cancel: true,
            engine_version: "0.7.22".to_string(),
            resolvable_providers: vec![meerkat_core::Provider::Anthropic],
            ..BridgeCapabilities::default()
        };
        let replies = vec![
            BridgeReply::BindHost(BridgeHostBindResponse {
                host_peer_id: "host-peer".to_string(),
                binding_generation: 1,
                address: "tcp://10.0.0.2:7100".to_string(),
                capabilities: capabilities.clone(),
                live_endpoint: Some("wss://host-b:7443/live".to_string()),
            }),
            BridgeReply::HostRebound(BridgeHostReboundResponse {
                host_peer_id: "host-peer".to_string(),
                binding_generation: 1,
                capabilities: capabilities.clone(),
                live_endpoint: None,
            }),
            BridgeReply::HostRevoked(BridgeHostRevokedResponse {
                host_peer_id: "host-peer".to_string(),
                mob_id: "mob-1".to_string(),
                epoch: 2,
                binding_generation: 1,
                released_members: vec!["worker-1".to_string()],
            }),
            BridgeReply::MemberHistoryPage(BridgeMemberHistoryPage {
                generation: 2,
                page: WireMemberHistoryPageBody {
                    from_index: 0,
                    messages: Vec::new(),
                    message_count: 0,
                    next_index: None,
                    complete: true,
                },
            }),
            BridgeReply::MemberEventsPage(BridgeMemberEventsPage {
                generation: 2,
                fence_token: 9,
                events: vec![WireEventRow {
                    durable_seq: 77,
                    envelope: meerkat_core::EventEnvelope::new(
                        "worker-1",
                        7,
                        Some("mob-1".to_string()),
                        meerkat_core::AgentEvent::TurnStarted { turn_number: 1 },
                    ),
                }],
                from_seq: 77,
                next_seq: 78,
                watermark: 90,
                turn_outcomes: vec![BridgeTurnOutcomeRecord {
                    input_id: "input-directed".to_string(),
                    generation: 2,
                    fence_token: 9,
                    terminal_seq: 77,
                    outcome: WireFlowTurnOutcome::ExtractionFailed {
                        detail: WireFlowFailureDetail::complete("schema mismatch".to_string()),
                    },
                }],
                outcomes_complete: true,
            }),
            BridgeReply::MemberMaterialized(BridgeMaterializedResponse {
                member_pubkey: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_string(),
                member_peer_id: "member-peer".to_string(),
                advertised_address: "tcp://10.0.0.2:7101".to_string(),
                session_id: "sess-9".to_string(),
                spec_digest: "d".repeat(64),
                engine_version: "0.7.22".to_string(),
                launch_outcome: MaterializeLaunchOutcome::ResumedFromSnapshot,
                resolved_auth_binding: None,
            }),
            BridgeReply::MemberReleased(BridgeMemberReleasedResponse {
                disposal: MemberSessionDisposal::RuntimeReleasedOnly {
                    cause: RuntimeReleaseCause::NoDurableSessions,
                },
            }),
            BridgeReply::HostStatus(BridgeHostStatusResponse {
                members: vec![BridgeHostMemberRecord {
                    agent_identity: "worker-1".to_string(),
                    generation: 1,
                    fence_token: 3,
                    session_id: "sess-9".to_string(),
                    spec_digest: "d".repeat(64),
                    healthy: true,
                }],
                capabilities,
            }),
            BridgeReply::MemberLiveChannelOpened(BridgeLiveOpenedResponse {
                open: LiveOpenResult {
                    channel_id: "chan-1".to_string(),
                    transport: crate::wire::WireLiveTransportBootstrap::Websocket {
                        url: "wss://host-b:7443/live/chan-1".to_string(),
                        token: "one-time-token".to_string(),
                    },
                    capabilities: crate::wire::WireLiveChannelCapabilities {
                        audio_in: true,
                        audio_out: true,
                        text_in: true,
                        text_out: true,
                        image_in: false,
                        video_in: false,
                        transcript_supported: true,
                        barge_in_supported: true,
                        provider_native_resume: false,
                    },
                    continuity: crate::wire::WireLiveContinuityMode::Fresh,
                },
            }),
            BridgeReply::MemberLiveChannelStatusReport {
                channel_id: "chan-1".to_string(),
                status: WireLiveAdapterStatus::Ready,
            },
            BridgeReply::MemberLiveChannelClosed {
                status: crate::wire::LiveCloseStatus::Closed,
            },
            BridgeReply::MemberLiveChannelControlled(BridgeLiveControlledResponse {
                outcome: BridgeLiveControlOutcome::Truncate {
                    status: crate::wire::LiveTruncateStatus::Truncated,
                },
            }),
            BridgeReply::MemberOperatorReply(MemberOperatorReply {
                request_id: "req-1".to_string(),
                outcome: MemberOperatorOutcome::Completed {
                    result: WireOpaqueJson::from_value(
                        &json!({"member_id": "worker-2", "status": "active"}),
                    ),
                },
            }),
            BridgeReply::MemberOperatorReply(MemberOperatorReply {
                request_id: "req-2".to_string(),
                outcome: MemberOperatorOutcome::Rejected {
                    cause: BridgeRejectionCause::ScopeDenied {
                        required: crate::wire::mob::WireControlScope::SendCommand,
                        presented: vec![],
                    },
                    reason: "spawn scope not granted".to_string(),
                },
            }),
        ];
        for reply in &replies {
            assert_reply_value_round_trip(reply);
        }
    }

    #[test]
    fn host_binding_descriptor_round_trips_and_fails_closed() {
        let descriptor = WireHostBindingDescriptor {
            kind: WireHostBindingDescriptorKind::Host,
            address: "tcp://10.0.0.2:7100".to_string(),
            identity: crate::wire::mob::WireTrustedPeerIdentity::Ed25519PublicKey {
                public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_string(),
            },
            bootstrap_token: "host-bootstrap".into(),
            live_endpoint: None,
        };
        let value = serde_json::to_value(&descriptor).expect("serialize descriptor");
        assert_eq!(value["kind"], json!("host"));
        assert!(
            value.get("live_endpoint").is_none(),
            "absent live endpoint must be omitted"
        );
        let decoded: WireHostBindingDescriptor =
            serde_json::from_value(value.clone()).expect("decode descriptor");
        assert_eq!(decoded, descriptor);

        let mut with_raw_peer = value;
        with_raw_peer["peer_id"] = json!("raw-peer-id");
        assert!(
            serde_json::from_value::<WireHostBindingDescriptor>(with_raw_peer).is_err(),
            "raw peer ids are not descriptor vocabulary — identity is the typed key"
        );
    }

    /// The opaque-JSON envelope is transparent on the wire (a JSON string
    /// whose content is serialized JSON), equality is byte equality of the
    /// stored form, and a hand-built non-JSON envelope fails typed at
    /// `to_value`.
    #[test]
    fn wire_opaque_json_round_trips_and_parses_back() {
        let value = json!({"member_id": "worker-2", "status": "active"});
        let envelope = WireOpaqueJson::from_value(&value);
        let wire = serde_json::to_value(&envelope).expect("serialize envelope");
        assert!(wire.is_string(), "envelope must be a transparent string");
        let decoded: WireOpaqueJson = serde_json::from_value(wire).expect("decode envelope");
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.to_value().expect("parse envelope body"), value);

        let garbage: WireOpaqueJson =
            serde_json::from_value(json!("not json at all")).expect("string decodes");
        assert!(
            garbage.to_value().is_err(),
            "non-JSON envelope bodies must fail typed at parse time"
        );
    }

    /// Event rows preserve the durable store seq separately from the
    /// envelope's source-stream seq, and compare the frozen envelope by
    /// semantic JSON equality.
    #[test]
    fn wire_event_row_equality_is_serialized_form_equality() {
        let row = WireEventRow {
            durable_seq: 41,
            envelope: meerkat_core::EventEnvelope::new(
                "worker-1",
                9,
                Some("mob-1".to_string()),
                meerkat_core::AgentEvent::TurnStarted { turn_number: 2 },
            ),
        };
        let wire = serde_json::to_value(&row).expect("serialize row");
        assert!(
            wire.get("durable_seq") == Some(&json!(41))
                && wire.get("envelope").and_then(|value| value.get("seq")) == Some(&json!(9)),
            "row must keep the durable and source-stream seq domains distinct: {wire}"
        );
        let decoded: WireEventRow = serde_json::from_value(wire).expect("decode row");
        assert_eq!(decoded, row, "round-tripped row must compare equal");

        let other = WireEventRow {
            durable_seq: 42,
            envelope: row.envelope.clone(),
        };
        assert_ne!(row, other, "different durable seq must compare unequal");
    }
}
