//! Realtime channel wire contracts.

use serde::{Deserialize, Serialize};

/// Canonical wire protocol version for realtime channels.
///
/// The version is bumped every time the on-the-wire shape of
/// [`RealtimeAudioChunk`], frame enums, or other load-bearing realtime
/// contracts changes in an incompatible way. Clients and servers handshake on
/// the string form; this enum is the single typed owner of the set.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum RealtimeProtocolVersion {
    /// v2: adds `sample_rate_hz` + `channels` to [`RealtimeAudioChunk`] and
    /// typed [`RealtimeErrorCode`] on channel errors.
    #[serde(rename = "2")]
    V2,
}

impl RealtimeProtocolVersion {
    /// The current wire protocol version shipped by this build.
    pub const CURRENT: Self = Self::V2;

    /// Every version this build is willing to accept on handshake.
    pub const SUPPORTED: &'static [Self] = &[Self::V2];

    /// Returns true when this version is one of `SUPPORTED`. Convenience
    /// wrapper for handshake helpers and tests.
    #[must_use]
    pub fn is_supported(self) -> bool {
        Self::SUPPORTED.contains(&self)
    }

    /// Render the protocol version to its stable string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V2 => "2",
        }
    }

    /// Parse a protocol version string, returning `None` for unknown values.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::SUPPORTED
            .iter()
            .copied()
            .find(|version| version.as_str() == raw)
    }
}

/// Typed realtime channel error code. Replaces the prior freeform `String` on
/// [`RealtimeChannelErrorFrame`]; all paths that mint a channel error pick
/// exactly one variant so downstream SDKs can match without string folklore.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeErrorCode {
    /// Incoming frame could not be parsed.
    InvalidFrame,
    /// The first frame on a new socket was not `channel.open`.
    ExpectedChannelOpen,
    /// The open token did not match any pending reservation.
    InvalidOpenToken,
    /// The open token was valid but its TTL elapsed before use.
    OpenTokenExpired,
    /// The client-requested role does not match the reservation.
    RoleMismatch,
    /// The client-requested turning mode does not match the reservation.
    TurningModeMismatch,
    /// The reservation requested a turning mode this host does not support.
    UnsupportedTurningMode,
    /// The target already has an active primary channel.
    TargetBusy,
    /// The client handshake requested a protocol version this host cannot serve.
    UnsupportedProtocolVersion,
    /// An input audio chunk's sample rate or channel count did not match the
    /// format the provider session is negotiated for.
    AudioFormatMismatch,
    /// The open token was minted for a different realm than the one the
    /// websocket session currently resolves to. Enforced server-side.
    UnauthorizedRealm,
    /// A tool dispatch exceeded its configured budget.
    ToolCallTimeout,
    /// Generic internal runtime error (fallback when no better code fits).
    InternalError,
    /// Reconnect budget exhausted; channel is closing.
    ReconnectExhausted,
    /// Realtime target did not parse or resolve.
    InvalidTarget,
    /// No realtime binding exists for this channel (driver not ready).
    ChannelNotBound,
    /// Runtime layer returned an internal error that has no more-specific code.
    RuntimeInternal,
    /// Runtime driver is not in a state that can accept this operation.
    RuntimeNotReady,
    /// Upstream provider session is closed.
    ProviderSessionClosed,
    /// Upstream provider session failed to open or progressed fatally.
    ProviderSessionFailed,
    /// Upstream provider session is temporarily unavailable.
    ProviderSessionUnavailable,
    /// Input modality is not supported by the negotiated capabilities.
    UnsupportedInputKind,
    /// No pending turn on this channel to commit or interrupt.
    NoPendingTurn,
    /// Observer channels may not send input or control frames.
    ObserverReadOnly,
    /// `channel.open` is valid only as the first frame on a new socket.
    UnexpectedChannelOpen,
    /// `channel.commit_turn` is only valid for explicit-commit turning mode.
    CommitTurnUnavailable,
    /// Channel is reconnecting; the caller must wait for a ready state.
    ChannelReconnecting,
    /// W3-H: the mob member this channel was bound to has been retired and
    /// its realtime binding released by the MobMachine
    /// (`MemberRealtimeBindingReleased` effect). Signalled only for
    /// `MobMember` targets; `SessionTarget` channels do not encounter this.
    BindingReleased,
}

impl RealtimeErrorCode {
    /// Stable string form; also the JSON discriminant.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidFrame => "invalid_frame",
            Self::ExpectedChannelOpen => "expected_channel_open",
            Self::InvalidOpenToken => "invalid_open_token",
            Self::OpenTokenExpired => "open_token_expired",
            Self::RoleMismatch => "role_mismatch",
            Self::TurningModeMismatch => "turning_mode_mismatch",
            Self::UnsupportedTurningMode => "unsupported_turning_mode",
            Self::TargetBusy => "target_busy",
            Self::UnsupportedProtocolVersion => "unsupported_protocol_version",
            Self::AudioFormatMismatch => "audio_format_mismatch",
            Self::UnauthorizedRealm => "unauthorized_realm",
            Self::ToolCallTimeout => "tool_call_timeout",
            Self::InternalError => "internal_error",
            Self::ReconnectExhausted => "reconnect_exhausted",
            Self::InvalidTarget => "invalid_target",
            Self::ChannelNotBound => "channel_not_bound",
            Self::RuntimeInternal => "runtime_internal",
            Self::RuntimeNotReady => "runtime_not_ready",
            Self::ProviderSessionClosed => "provider_session_closed",
            Self::ProviderSessionFailed => "provider_session_failed",
            Self::ProviderSessionUnavailable => "provider_session_unavailable",
            Self::UnsupportedInputKind => "unsupported_input_kind",
            Self::NoPendingTurn => "no_pending_turn",
            Self::ObserverReadOnly => "observer_read_only",
            Self::UnexpectedChannelOpen => "unexpected_channel_open",
            Self::CommitTurnUnavailable => "commit_turn_unavailable",
            Self::ChannelReconnecting => "channel_reconnecting",
            Self::BindingReleased => "binding_released",
        }
    }
}

impl std::fmt::Display for RealtimeErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Descriptor for an expected or actual realtime audio format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeAudioFormat {
    /// IANA-style MIME type, e.g. `audio/pcm`.
    pub mime_type: String,
    /// Sample rate in hertz, e.g. `24000`.
    pub sample_rate_hz: u32,
    /// Channel count; `1` for mono.
    pub channels: u8,
}

impl RealtimeAudioFormat {
    /// Construct an `audio/pcm` format descriptor.
    #[must_use]
    pub fn pcm(sample_rate_hz: u32, channels: u8) -> Self {
        Self {
            mime_type: "audio/pcm".to_string(),
            sample_rate_hz,
            channels,
        }
    }
}

/// Typed context carried alongside a [`RealtimeErrorCode::AudioFormatMismatch`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AudioFormatMismatchContext {
    pub expected: RealtimeAudioFormat,
    pub actual: RealtimeAudioFormat,
}

/// Typed context carried alongside a [`RealtimeErrorCode::ToolCallTimeout`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolCallTimeoutContext {
    pub call_id: String,
    pub elapsed_ms: u64,
    pub timeout_ms: u64,
}

/// Target for a public realtime channel.
///
/// Two variants, one for each addressing mode:
///
/// - `SessionTarget` — standalone sessions (no mob-member continuity). The
///   session id is pinned for the channel's lifetime; when that session
///   ends, the channel ends.
///
/// - `MobMember` — mob-member continuity (W3-H / dogma #4). Identity is the
///   canonical anchor, and the server resolves the current bridge session
///   on every tick from the MobMachine's `member_realtime_bindings` map.
///   Respawn atomically rotates the bound session via the
///   `MemberRealtimeBindingRotated` effect; the channel survives without
///   any SDK round-trip. A terminal `MemberRealtimeBindingReleased`
///   closes the channel with `RealtimeErrorCode::BindingReleased`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeChannelTarget {
    SessionTarget {
        session_id: String,
    },
    MobMember {
        mob_id: String,
        agent_identity: String,
    },
}

/// Opening role for a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeChannelRole {
    Primary,
    Observer,
}

/// Turning mode for a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTurningMode {
    ProviderManaged,
    ExplicitCommit,
}

/// Input modality kind supported by a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeInputKind {
    Text,
    Audio,
    Video,
}

/// Output modality kind supported by a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeOutputKind {
    Text,
    Audio,
    Video,
}

/// Public reconnect policy for a realtime channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeReconnectPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub max_total_ms: u64,
}

/// Per-channel runtime knobs negotiated at open time.
///
/// Additive fields only — clients that do not carry this struct inherit the
/// server-default behavior via `#[serde(default)]` on the parent
/// [`RealtimeOpenRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelConfig {
    /// Maximum wall-clock time a tool dispatch may take before the channel
    /// gives up and injects a synthetic tool-error result. `None` disables the
    /// deadline (product explicitly opted into unlimited tools); omitting the
    /// field leaves the server default in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
}

impl RealtimeChannelConfig {
    /// Default tool budget when neither the caller nor the server provides
    /// an override. Keeps the "runtime safe by default" dogma.
    pub const DEFAULT_TOOL_TIMEOUT_MS: u64 = 15_000;

    /// Resolve the effective tool timeout, substituting the global default
    /// when the caller did not specify one. `None` means "no deadline" and is
    /// honored exactly.
    #[must_use]
    pub fn tool_timeout_ms_or_default(&self) -> Option<u64> {
        match self.tool_timeout_ms {
            Some(0) => None,
            Some(ms) => Some(ms),
            None => Some(Self::DEFAULT_TOOL_TIMEOUT_MS),
        }
    }
}

/// Product-facing realtime capability set for one target/provider combination.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_kinds: Vec<RealtimeInputKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_kinds: Vec<RealtimeOutputKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turning_modes: Vec<RealtimeTurningMode>,
    pub interrupt_supported: bool,
    pub transcript_supported: bool,
    pub tool_lifecycle_events_supported: bool,
    pub video_supported: bool,
    /// Audio format the provider session accepts for client audio input.
    /// Clients MUST stamp incoming [`RealtimeAudioChunk`]s with the same
    /// `sample_rate_hz` and `channels`; mismatches are rejected server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_input_format: Option<RealtimeAudioFormat>,
    /// Audio format the provider session emits for output audio chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_output_format: Option<RealtimeAudioFormat>,
}

/// Lifecycle state for a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeChannelState {
    Opening,
    Ready,
    Interrupted,
    Reconnecting,
    Closed,
    Error,
}

/// Public realtime channel status projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelStatus {
    pub state: RealtimeChannelState,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    /// Wall-clock deadline after which the reconnect cycle will be abandoned
    /// and the channel will close with `ReconnectExhausted`. Derived from the
    /// reconnect policy's `max_total_ms` on top of the cycle's start time. RFC
    /// 3339 encoded, matches `next_retry_at`. Present only while the channel
    /// is actively reconnecting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Request payload for `realtime/open_info`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeOpenRequest {
    pub target: RealtimeChannelTarget,
    pub role: RealtimeChannelRole,
    pub turning_mode: RealtimeTurningMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_policy: Option<RealtimeReconnectPolicy>,
    /// Optional per-channel runtime knobs (tool budget, etc.). Omitted means
    /// "use server defaults".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_config: Option<RealtimeChannelConfig>,
}

/// Response payload for `realtime/open_info`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeOpenInfo {
    pub ws_url: String,
    pub open_token: String,
    pub expires_at: String,
    pub target: RealtimeChannelTarget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_protocol_versions: Vec<String>,
    pub default_protocol_version: String,
    pub capabilities: RealtimeCapabilities,
}

/// Request payload for `realtime/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeStatusParams {
    pub target: RealtimeChannelTarget,
}

/// Response payload for `realtime/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeStatusResult {
    pub status: RealtimeChannelStatus,
}

/// Request payload for `realtime/capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeCapabilitiesParams {
    pub target: RealtimeChannelTarget,
}

/// Response payload for `realtime/capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeCapabilitiesResult {
    pub capabilities: RealtimeCapabilities,
}

/// A text chunk for realtime ingress/egress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeTextChunk {
    pub text: String,
}

/// A text delta chunk for realtime output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeTextDelta {
    pub delta: String,
}

/// An opaque realtime audio chunk with MIME + format metadata.
///
/// Both sender and receiver MUST stamp `sample_rate_hz` and `channels` so the
/// transport layer can validate against the provider session's negotiated
/// format instead of silently producing garbled audio when an ESP32 or browser
/// client ships the wrong rate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeAudioChunk {
    pub mime_type: String,
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub data: String,
}

impl RealtimeAudioChunk {
    /// Extract the structured format descriptor for validation comparisons.
    #[must_use]
    pub fn format(&self) -> RealtimeAudioFormat {
        RealtimeAudioFormat {
            mime_type: self.mime_type.clone(),
            sample_rate_hz: self.sample_rate_hz,
            channels: self.channels,
        }
    }
}

/// An opaque realtime video chunk with MIME metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeVideoChunk {
    pub mime_type: String,
    pub data: String,
}

/// Modality-neutral input chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeInputChunk {
    TextChunk(RealtimeTextChunk),
    AudioChunk(RealtimeAudioChunk),
    VideoChunk(RealtimeVideoChunk),
}

/// Modality-neutral output chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeOutputChunk {
    TextDelta(RealtimeTextDelta),
    AudioChunk(RealtimeAudioChunk),
    VideoChunk(RealtimeVideoChunk),
}

/// Normalized realtime event stream payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeEvent {
    InputTranscriptPartial {
        text: String,
    },
    InputTranscriptFinal {
        text: String,
        /// Opaque, provider-specific prosody annotation (tone / sentiment /
        /// pitch hints). Additive field. Providers that do not surface
        /// structured prosody leave it `None`; compactors and downstream
        /// tools may use this as a hint only — the authoritative transcript
        /// is still `text`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prosody_hint: Option<String>,
    },
    TurnStarted,
    TurnCommitted,
    TurnCompleted,
    OutputTextDelta {
        delta: String,
    },
    OutputAudioChunk {
        chunk: RealtimeAudioChunk,
    },
    OutputVideoChunk {
        chunk: RealtimeVideoChunk,
    },
    Interrupted,
    ToolCallRequested {
        call_id: String,
        tool_name: String,
    },
    ToolCallCompleted {
        call_id: String,
    },
    ToolCallFailed {
        call_id: String,
        error: String,
    },
    /// The tool dispatch exceeded its configured budget and was aborted;
    /// a synthetic `ToolResult::Error` was injected back into the provider
    /// session so the model sees a concrete failure instead of silence.
    ToolCallTimedOut {
        call_id: String,
        elapsed_ms: u64,
    },
    /// The assistant output for `item_id` was truncated at
    /// `audio_played_ms` because the user barged in. `truncated_text` is the
    /// transcript prefix the user actually heard, as reported by the
    /// provider; when the provider has not yet supplied a re-projected
    /// transcript the field is omitted (`None`) and downstream projectors
    /// should keep the existing transcript until a later emission fills it in.
    AssistantTranscriptTruncated {
        item_id: String,
        audio_played_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        truncated_text: Option<String>,
    },
    StatusChanged {
        status: RealtimeChannelStatus,
    },
    NeedsReattach,
}

/// Payload for `channel.open`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelOpenFrame {
    pub protocol_version: String,
    pub open_token: String,
    pub role: RealtimeChannelRole,
    pub turning_mode: RealtimeTurningMode,
}

/// Payload for `channel.input`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelInputFrame {
    pub chunk: RealtimeInputChunk,
}

/// Payload for `channel.opened`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelOpenedFrame {
    pub protocol_version: String,
    pub status: RealtimeChannelStatus,
    pub capabilities: RealtimeCapabilities,
    pub role: RealtimeChannelRole,
}

/// Payload for `channel.status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelStatusFrame {
    pub status: RealtimeChannelStatus,
}

/// Payload for `channel.event`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelEventFrame {
    pub event: RealtimeEvent,
}

/// Structured typed context carried on a channel error frame. Each variant
/// corresponds to a specific [`RealtimeErrorCode`] and lets clients match
/// the reason without parsing the message string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeErrorDetails {
    AudioFormatMismatch(AudioFormatMismatchContext),
    ToolCallTimeout(ToolCallTimeoutContext),
    UnsupportedProtocolVersion {
        requested: String,
        supported: Vec<String>,
    },
}

/// Payload for `channel.error`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelErrorFrame {
    /// Typed error code. Single source of truth for branching in SDKs.
    pub code: RealtimeErrorCode,
    /// Human-readable message suitable for logs and operator surfaces.
    pub message: String,
    /// Structured context keyed to the code, when additional fields are useful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<RealtimeErrorDetails>,
}

/// Payload for `channel.closed`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelClosedFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Payload for `channel.barge_in_truncate`.
///
/// Sent by the client the moment it detects the user starting to speak over
/// the assistant's audio, to tell the server what prefix was actually heard.
/// Field names mirror OpenAI Realtime's `conversation.item.truncate` so the
/// provider adapter can map straight through without re-encoding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeBargeInTruncateFrame {
    /// Assistant item whose output is being truncated. Matches the `item_id`
    /// emitted in the public `RealtimeEvent::AssistantTranscriptTruncated`.
    pub item_id: String,
    /// Content index within the item (0 for the primary audio/text stream).
    pub content_index: u32,
    /// Milliseconds of assistant audio the client actually played back
    /// before the user interrupted. Must not exceed the true elapsed duration
    /// of the audio content.
    pub audio_played_ms: u64,
}

/// Client-to-server realtime frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
pub enum RealtimeClientFrame {
    #[serde(rename = "channel.open")]
    ChannelOpen(RealtimeChannelOpenFrame),
    #[serde(rename = "channel.input")]
    ChannelInput(RealtimeChannelInputFrame),
    #[serde(rename = "channel.commit_turn")]
    ChannelCommitTurn,
    #[serde(rename = "channel.interrupt")]
    ChannelInterrupt,
    /// Report a client-side barge-in with the playback cursor, so the server
    /// can truncate the canonical transcript to what the user actually heard.
    #[serde(rename = "channel.barge_in_truncate")]
    ChannelBargeInTruncate(RealtimeBargeInTruncateFrame),
    #[serde(rename = "channel.close")]
    ChannelClose,
}

/// Server-to-client realtime frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
pub enum RealtimeServerFrame {
    #[serde(rename = "channel.opened")]
    ChannelOpened(RealtimeChannelOpenedFrame),
    #[serde(rename = "channel.status")]
    ChannelStatus(RealtimeChannelStatusFrame),
    #[serde(rename = "channel.event")]
    ChannelEvent(RealtimeChannelEventFrame),
    #[serde(rename = "channel.error")]
    ChannelError(RealtimeChannelErrorFrame),
    #[serde(rename = "channel.closed")]
    ChannelClosed(RealtimeChannelClosedFrame),
}
