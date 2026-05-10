//! `live/*` RPC wire contracts.
//!
//! Typed parameter and result shapes for the `live/open`, `live/status`,
//! `live/close`, `live/send_input`, `live/commit_input`, `live/interrupt`,
//! and `live/truncate` JSON-RPC methods. Promoted from
//! `meerkat-rpc/src/handlers/live.rs` (I52) so SDKs and the protocol catalog
//! can reference them as typed contracts rather than free-form `basic`
//! entries.
//!
//! Result shapes that carry semantic adapter state (`transport`,
//! `capabilities`, `continuity`, `status`) reference the canonical types in
//! `meerkat_core::live_adapter`. The RPC catalog publishes the types under
//! the `LiveOpenResult` / `LiveStatusResult` titles so SDK codegen sees a
//! single source of truth.

use serde::{Deserialize, Serialize};

use meerkat_core::live_adapter::{
    LiveAdapterErrorCode, LiveAdapterObservation, LiveAdapterStatus, LiveChannelCapabilities,
    LiveContinuityMode, LiveDegradationReason, LiveResponseModality, LiveTransportBootstrap,
};
use meerkat_core::realtime_transcript::RealtimeTranscriptEvent;
use meerkat_core::types::Usage;

use crate::wire::session::WireStopReason;

/// Request payload for `live/open`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveOpenParams {
    pub session_id: String,
}

/// Response payload for `live/open`.
///
/// `capabilities`, `continuity`, and `transport` are typed wire-side mirrors
/// of the core `LiveChannelCapabilities` / `LiveContinuityMode` /
/// `LiveTransportBootstrap` so SDK codegen sees the real shape (typed
/// booleans, internally-tagged variant payloads, discriminated transport
/// union) instead of an opaque JSON blob. CC5/CC6 (PR #650 verifier
/// follow-up); G8 (P2): `transport` typed-mirror.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveOpenResult {
    pub channel_id: String,
    pub transport: WireLiveTransportBootstrap,
    pub capabilities: WireLiveChannelCapabilities,
    pub continuity: WireLiveContinuityMode,
}

/// Wire projection of [`meerkat_core::live_adapter::LiveTransportBootstrap`].
///
/// Internally-tagged on `transport` (snake_case) — matches the core enum's
/// serde shape exactly so the wire payload is byte-identical. G8 (P2):
/// closes the typed-surface gap that left `transport: unknown` in TS and
/// `Any` in Python at the SDK boundary.
///
/// The core enum is `#[non_exhaustive]`; new transports (e.g. WebRTC
/// reintroduction per Round-4 T4) appear here as additional typed variants
/// rather than as a free-form JSON blob.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "transport", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveTransportBootstrap {
    /// WebSocket bootstrap: client opens a WS connection to `url` and
    /// authenticates with `token`. Mirrors
    /// [`meerkat_core::live_adapter::LiveTransportBootstrap::Websocket`].
    Websocket { url: String, token: String },
}

impl From<LiveTransportBootstrap> for WireLiveTransportBootstrap {
    fn from(value: LiveTransportBootstrap) -> Self {
        match value {
            LiveTransportBootstrap::Websocket { url, token } => Self::Websocket { url, token },
            // Core enum is `#[non_exhaustive]`; debug-assert for new variants
            // and fall back to an empty Websocket placeholder. When a new
            // variant lands, add an explicit arm above this comment.
            _ => {
                debug_assert!(
                    false,
                    "WireLiveTransportBootstrap::from saw an unmapped \
                     LiveTransportBootstrap variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Websocket {
                    url: String::new(),
                    token: String::new(),
                }
            }
        }
    }
}

impl From<WireLiveTransportBootstrap> for LiveTransportBootstrap {
    fn from(value: WireLiveTransportBootstrap) -> Self {
        // No wildcard arm: `WireLiveTransportBootstrap` is owned by this
        // crate so even with `#[non_exhaustive]` (which only constrains
        // matches outside the defining crate) the compiler enforces
        // exhaustive coverage here. Adding a new wire variant therefore
        // forces a new arm in this match — no silent fallthrough.
        match value {
            WireLiveTransportBootstrap::Websocket { url, token } => Self::Websocket { url, token },
        }
    }
}

/// Wire projection of [`meerkat_core::live_adapter::LiveChannelCapabilities`].
///
/// Typed-boolean matrix advertised when a live channel opens. SDK consumers
/// (Python `client.py`, TypeScript `client.ts`, Web SDK) get typed access to
/// `image_in` / `video_in` / `transcript_supported` etc. without needing to
/// hand-decode an opaque JSON object. CC5: closes the typed-surface gap that
/// hid T8's "anticipate `gpt-realtime-2`" goal at the SDK boundary.
///
/// Field shape mirrors the core type 1:1 — adding a new capability requires
/// extending both the core type and this mirror; the `From` impls below
/// fail-closed if a field is dropped (compile error: missing field). New
/// modalities appear here as additional typed booleans, never as
/// stringly-typed lists or provider-specific enums.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLiveChannelCapabilities {
    /// Adapter accepts audio chunks via `live/send_input`.
    pub audio_in: bool,
    /// Adapter emits audio chunks via `AssistantAudioChunk` observations.
    pub audio_out: bool,
    /// Adapter accepts text chunks via `live/send_input`.
    pub text_in: bool,
    /// Adapter emits display text via `AssistantTextDelta` observations.
    pub text_out: bool,
    /// Adapter accepts image input via `live/send_input` (e.g. future
    /// `gpt-realtime-2` image support). Today: `false` for OpenAI realtime.
    pub image_in: bool,
    /// Adapter accepts video-frame input via `live/send_input` (e.g.
    /// Gemini Live). Today: `false` for OpenAI realtime.
    pub video_in: bool,
    /// Adapter emits spoken-audio transcripts via
    /// `AssistantTranscriptDelta` / `AssistantTranscriptFinal`.
    pub transcript_supported: bool,
    /// Adapter supports user-initiated barge-in (turn truncation) via
    /// `live/interrupt` and the `TurnInterrupted` observation.
    pub barge_in_supported: bool,
    /// Adapter can resume a prior provider-side session by id (transcript-
    /// only resume does not count). `false` until a provider exposes a real
    /// continuation handle.
    pub provider_native_resume: bool,
}

impl From<LiveChannelCapabilities> for WireLiveChannelCapabilities {
    fn from(value: LiveChannelCapabilities) -> Self {
        let LiveChannelCapabilities {
            audio_in,
            audio_out,
            text_in,
            text_out,
            image_in,
            video_in,
            transcript_supported,
            barge_in_supported,
            provider_native_resume,
        } = value;
        Self {
            audio_in,
            audio_out,
            text_in,
            text_out,
            image_in,
            video_in,
            transcript_supported,
            barge_in_supported,
            provider_native_resume,
        }
    }
}

impl From<WireLiveChannelCapabilities> for LiveChannelCapabilities {
    fn from(value: WireLiveChannelCapabilities) -> Self {
        let WireLiveChannelCapabilities {
            audio_in,
            audio_out,
            text_in,
            text_out,
            image_in,
            video_in,
            transcript_supported,
            barge_in_supported,
            provider_native_resume,
        } = value;
        Self {
            audio_in,
            audio_out,
            text_in,
            text_out,
            image_in,
            video_in,
            transcript_supported,
            barge_in_supported,
            provider_native_resume,
        }
    }
}

/// Wire projection of [`meerkat_core::live_adapter::LiveContinuityMode`].
///
/// Internally-tagged on `mode` (snake_case) — matches the core enum's serde
/// shape exactly so the wire payload is byte-identical. CC6: closes the
/// typed-surface gap. SDK consumers get a discriminated union (TS) or
/// tagged-variant (Python) instead of a raw JSON blob, and the
/// `ProviderNativeResume { provider_session_id }` payload field is visible
/// to schema codegen.
///
/// **Breaking-change note (pre-1.0 dogma, no shims):** T12 already moved the
/// core enum from a bare-string serde shape (`"transcript_only"`) to the
/// internally-tagged form (`{"mode":"transcript_only"}`). This wire mirror
/// makes that shape change visible to schema codegen and SDK clients.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveContinuityMode {
    /// Brand-new live channel; no prior session history.
    Fresh,
    /// History seeded from canonical transcript only. Honest about loss of
    /// AEC / playback-cursor / pronunciation / provider-native state.
    TranscriptOnly,
    /// History seeded but with known gaps.
    Degraded,
    /// Provider surfaced a session id we can attach back to. Only mode that
    /// preserves provider-native state across reconnects. No provider
    /// Meerkat ships today returns a usable resume id.
    ProviderNativeResume { provider_session_id: String },
}

impl From<LiveContinuityMode> for WireLiveContinuityMode {
    fn from(value: LiveContinuityMode) -> Self {
        match value {
            LiveContinuityMode::Fresh => Self::Fresh,
            LiveContinuityMode::TranscriptOnly => Self::TranscriptOnly,
            LiveContinuityMode::Degraded => Self::Degraded,
            LiveContinuityMode::ProviderNativeResume {
                provider_session_id,
            } => Self::ProviderNativeResume {
                provider_session_id,
            },
            // CC6: `LiveContinuityMode` is `non_exhaustive`, so the
            // compiler requires a wildcard. Same compromise as
            // `WireTranscriptSource::from` (`wire/session.rs`):
            // `meerkat-contracts` does not depend on `tracing`, the no-panic
            // rule forbids `unreachable!()`, and a `TryFrom` would force
            // every call site to handle a variant we don't yet have. Use
            // `debug_assert!(false, ...)` so debug/CI builds fail loudly
            // when a new variant lands without an explicit arm; release
            // builds fall back to `Fresh` (the lowest-fidelity variant)
            // rather than producing UB. **When a new variant is added, add
            // an explicit arm above this comment.**
            _ => {
                debug_assert!(
                    false,
                    "WireLiveContinuityMode::from saw an unmapped \
                     LiveContinuityMode variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Fresh
            }
        }
    }
}

impl From<WireLiveContinuityMode> for LiveContinuityMode {
    fn from(value: WireLiveContinuityMode) -> Self {
        // No wildcard arm: `WireLiveContinuityMode` is owned by this crate
        // so even with `#[non_exhaustive]` (which only constrains matches
        // outside the defining crate) the compiler enforces exhaustive
        // coverage here. Adding a new wire variant therefore forces a new
        // arm in this match — no silent fallthrough.
        match value {
            WireLiveContinuityMode::Fresh => Self::Fresh,
            WireLiveContinuityMode::TranscriptOnly => Self::TranscriptOnly,
            WireLiveContinuityMode::Degraded => Self::Degraded,
            WireLiveContinuityMode::ProviderNativeResume {
                provider_session_id,
            } => Self::ProviderNativeResume {
                provider_session_id,
            },
        }
    }
}

/// Request payload for `live/status`, `live/close`, and `live/interrupt`.
/// They all take the same `{channel_id}` shape; this struct is the typed
/// name for it.
///
/// `live/commit_input` no longer uses this shape — it carries an optional
/// `response_modality` override (G9) so callers can request a text-only
/// response on an audio-first channel without flipping the channel
/// modality. See [`LiveCommitInputParams`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveChannelParams {
    pub channel_id: String,
}

/// Wire projection of [`meerkat_core::live_adapter::LiveResponseModality`].
///
/// Internally-tagged on `modality` (snake_case) — matches the core enum's
/// serde shape exactly so the wire payload is byte-identical. G9: closes
/// the typed-surface gap so SDK clients can pick `audio` vs `text` on a
/// per-response basis without round-tripping through a free-form string.
///
/// The core enum is `#[non_exhaustive]`; new modalities (e.g. structured
/// output, image) appear here as additional typed variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "modality", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveResponseModality {
    /// Spoken-audio plus the audio-derived transcript.
    Audio,
    /// Display text only — no audio output, no transcript.
    Text,
}

impl From<LiveResponseModality> for WireLiveResponseModality {
    fn from(value: LiveResponseModality) -> Self {
        match value {
            LiveResponseModality::Audio => Self::Audio,
            LiveResponseModality::Text => Self::Text,
            // Core enum is `#[non_exhaustive]`. Debug-assert on unmapped
            // variants and fall back to the audio-first default.
            _ => {
                debug_assert!(
                    false,
                    "WireLiveResponseModality::from saw an unmapped \
                     LiveResponseModality variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Audio
            }
        }
    }
}

impl From<WireLiveResponseModality> for LiveResponseModality {
    fn from(value: WireLiveResponseModality) -> Self {
        match value {
            WireLiveResponseModality::Audio => Self::Audio,
            WireLiveResponseModality::Text => Self::Text,
        }
    }
}

/// Request payload for `live/commit_input`.
///
/// G9: optional `response_modality` lets the caller request a text-only
/// response on an otherwise-audio channel without flipping the channel
/// modality. `None` keeps the channel default (`audio` for the OpenAI
/// realtime adapter today).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveCommitInputParams {
    pub channel_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_modality: Option<WireLiveResponseModality>,
}

/// Response payload for `live/status`. See `LiveOpenResult` for the
/// rationale on the schema-side opaque projection of the core type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveStatusResult {
    pub channel_id: String,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub status: LiveAdapterStatus,
}

/// Modality-tagged input chunk for `live/send_input`.
///
/// Audio / image / video-frame payloads are base64 strings (`data`); the
/// modality-specific metadata (`sample_rate_hz` / `channels` for audio,
/// `mime` for image, `codec` / `timestamp_ms` for video frames) lets the
/// adapter validate against the negotiated provider format.
///
/// T11: `Image` and `VideoFrame` mirror the typed variants on
/// [`meerkat_core::live_adapter::LiveInputChunk`]. Adapters that do not
/// implement a variant must reject with a typed
/// `LiveAdapterErrorCode::ConfigRejected` rather than collapsing onto a
/// free-form provider error string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveInputChunkWire {
    Audio {
        data: String,
        sample_rate_hz: u32,
        channels: u16,
    },
    Text {
        text: String,
    },
    Image {
        mime: String,
        data: String,
    },
    VideoFrame {
        codec: String,
        data: String,
        timestamp_ms: u64,
    },
}

/// Request payload for `live/send_input`.
///
/// **`BREAKING_LIVE_WIRE_FORMAT_V1`** (H48): `chunk` is a nested object, not
/// a flattened sibling of `channel_id`. WS protocol clients that piggyback on
/// this shape must use the nested form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveSendInputParams {
    pub channel_id: String,
    pub chunk: LiveInputChunkWire,
}

/// Request payload for `live/truncate`.
///
/// `item_id` and `content_index` are the provider-side handle for the
/// assistant item being truncated; `audio_played_ms` is the client-tracked
/// playback cursor at the moment of truncation. There is no server-side
/// playback-cursor read API — clients track playback locally and pass the
/// cursor in here when they want to truncate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveTruncateParams {
    pub channel_id: String,
    pub item_id: String,
    pub content_index: u32,
    pub audio_played_ms: u64,
}

// ---------------------------------------------------------------------------
// FIX-SDK-OBS — typed wire mirror for `LiveAdapterObservation`
// ---------------------------------------------------------------------------

/// Wire mirror of [`meerkat_core::live_adapter::LiveDegradationReason`].
///
/// Internally-tagged on `kind` (snake_case) — matches the core enum's serde
/// shape exactly. SDK consumers route on `kind` to distinguish the
/// non-payload variants from the typed `other { detail }` payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveDegradationReason {
    RateLimited,
    ProviderThrottled,
    NetworkUnstable,
    Other { detail: String },
}

impl From<LiveDegradationReason> for WireLiveDegradationReason {
    fn from(value: LiveDegradationReason) -> Self {
        match value {
            LiveDegradationReason::RateLimited => Self::RateLimited,
            LiveDegradationReason::ProviderThrottled => Self::ProviderThrottled,
            LiveDegradationReason::NetworkUnstable => Self::NetworkUnstable,
            LiveDegradationReason::Other { detail } => Self::Other {
                detail: detail.into_owned(),
            },
            // Core enum is `non_exhaustive`; debug-assert for new variants.
            _ => {
                debug_assert!(
                    false,
                    "WireLiveDegradationReason::from saw an unmapped \
                     LiveDegradationReason variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Other {
                    detail: "unknown".to_string(),
                }
            }
        }
    }
}

/// Wire mirror of [`meerkat_core::live_adapter::LiveAdapterStatus`].
///
/// Internally-tagged on `status` (snake_case). The `degraded` variant
/// references [`WireLiveDegradationReason`] so the typed reason is visible at
/// the SDK boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveAdapterStatus {
    Idle,
    Opening,
    Ready,
    Degraded { reason: WireLiveDegradationReason },
    Closing,
    Closed,
}

impl From<LiveAdapterStatus> for WireLiveAdapterStatus {
    fn from(value: LiveAdapterStatus) -> Self {
        match value {
            LiveAdapterStatus::Idle => Self::Idle,
            LiveAdapterStatus::Opening => Self::Opening,
            LiveAdapterStatus::Ready => Self::Ready,
            LiveAdapterStatus::Degraded { reason } => Self::Degraded {
                reason: reason.into(),
            },
            LiveAdapterStatus::Closing => Self::Closing,
            LiveAdapterStatus::Closed => Self::Closed,
            _ => {
                debug_assert!(
                    false,
                    "WireLiveAdapterStatus::from saw an unmapped \
                     LiveAdapterStatus variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Closed
            }
        }
    }
}

/// Wire mirror of [`meerkat_core::live_adapter::LiveAdapterErrorCode`].
///
/// Internally-tagged on `code` (snake_case). SDK consumers route on `code`
/// to distinguish payload-less variants from typed payload variants
/// (`config_rejected { reason }`, `other { raw }`). FIX-SDK-OBS: makes the
/// R5-9 `CommandRejected` observation's typed code visible at the SDK
/// boundary instead of a free-form blob.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveAdapterErrorCode {
    ConnectionFailed,
    ConnectionLost,
    ConfigRejected { reason: String },
    ProviderError,
    AuthenticationFailed,
    InternalError,
    Other { raw: String },
}

impl From<LiveAdapterErrorCode> for WireLiveAdapterErrorCode {
    fn from(value: LiveAdapterErrorCode) -> Self {
        match value {
            LiveAdapterErrorCode::ConnectionFailed => Self::ConnectionFailed,
            LiveAdapterErrorCode::ConnectionLost => Self::ConnectionLost,
            LiveAdapterErrorCode::ConfigRejected { reason } => Self::ConfigRejected { reason },
            LiveAdapterErrorCode::ProviderError => Self::ProviderError,
            LiveAdapterErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            LiveAdapterErrorCode::InternalError => Self::InternalError,
            LiveAdapterErrorCode::Other { raw } => Self::Other { raw },
            _ => {
                debug_assert!(
                    false,
                    "WireLiveAdapterErrorCode::from saw an unmapped \
                     LiveAdapterErrorCode variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::InternalError
            }
        }
    }
}

impl From<WireLiveAdapterErrorCode> for LiveAdapterErrorCode {
    fn from(value: WireLiveAdapterErrorCode) -> Self {
        match value {
            WireLiveAdapterErrorCode::ConnectionFailed => Self::ConnectionFailed,
            WireLiveAdapterErrorCode::ConnectionLost => Self::ConnectionLost,
            WireLiveAdapterErrorCode::ConfigRejected { reason } => Self::ConfigRejected { reason },
            WireLiveAdapterErrorCode::ProviderError => Self::ProviderError,
            WireLiveAdapterErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            WireLiveAdapterErrorCode::InternalError => Self::InternalError,
            WireLiveAdapterErrorCode::Other { raw } => Self::Other { raw },
        }
    }
}

/// Wire mirror of [`meerkat_core::live_adapter::LiveAdapterObservation`].
///
/// FIX-SDK-OBS: closes the R5-4 verifier gap. The core enum is the canonical
/// shape adapters emit, but it is not registered for schema emission and is
/// therefore invisible at the SDK boundary — browser/Python clients receive
/// observations as untyped JSON and cannot type-narrow on
/// `assistant_audio_chunk` (to read the new `item_id` / `response_id` /
/// `content_index` fields driving `live/truncate`) or on `command_rejected`
/// (a typed channel-survives error introduced in R5-9). The wire mirror
/// makes every variant visible to schema codegen and produces a discriminated
/// TypeScript union / typed Python `TypedDict` union.
///
/// Serde shape mirrors the core enum exactly: internally-tagged on
/// `observation` (snake_case). Round-trip with the core type is byte-
/// identical (see `wire_live_adapter_observation_byte_compatible_with_core`).
///
/// Field types reference other wire mirrors where they exist
/// ([`WireStopReason`], [`WireUsage`], [`WireLiveAdapterStatus`],
/// [`WireLiveAdapterErrorCode`]) and the canonical
/// [`RealtimeTranscriptEvent`] (which already derives `JsonSchema` and is
/// auto-promoted by the SDK codegen `Realtime*` allowlist rule).
///
/// Audio data is base64-encoded on the wire (matches the core
/// [`LiveAdapterObservation::AssistantAudioChunk`] base64 mode); the wire
/// mirror carries `data` as a `String` so the schema emits `String` instead
/// of an opaque `Vec<u8>` JSON-array shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "observation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveAdapterObservation {
    Ready,
    UserTranscriptFinal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_index: Option<u32>,
        text: String,
    },
    AssistantTextDelta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta_id: Option<String>,
        delta: String,
    },
    AssistantTranscriptDelta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta_id: Option<String>,
        delta: String,
    },
    /// R5-4: identity fields (`response_id`, `item_id`, `content_index`)
    /// propagate the source server-event identity so clients can attach a
    /// playback cursor to a provider item without racing on the
    /// transcript-delta arrival order.
    AssistantAudioChunk {
        /// Base64-encoded PCM payload. Matches the core
        /// [`LiveAdapterObservation::AssistantAudioChunk`] serde shape.
        data: String,
        sample_rate_hz: u32,
        channels: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_index: Option<u32>,
    },
    AssistantTranscriptFinal {
        provider_item_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        text: String,
        stop_reason: WireStopReason,
        // Core `Usage` does not have a registered schema mirror that the
        // SDK codegen promotes; expose as opaque JSON in the schema layer
        // (matches the `LiveOpenResult.transport: LiveTransportBootstrap`
        // pattern). Real shape is `{input_tokens, output_tokens,
        // cache_creation_tokens?, cache_read_tokens?}` per
        // `meerkat_core::types::Usage`.
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        usage: Usage,
    },
    AssistantTranscriptTruncated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Pass-through of a structured `RealtimeTranscriptEvent` from the
    /// provider. The core type already derives `JsonSchema` and the SDK
    /// codegen auto-promotes `Realtime*` schemas, so the typed shape lands
    /// in generated SDK types without an additional wire mirror.
    RealtimeTranscript {
        event: RealtimeTranscriptEvent,
    },
    ToolCallRequested {
        provider_call_id: String,
        tool_name: String,
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        arguments: serde_json::Value,
    },
    /// Barge-in: the user interrupted the assistant mid-turn.
    ///
    /// G4 (P1): `response_id` carries the in-flight provider response id so
    /// downstream consumers can scope the truncation to the right response
    /// even when the interrupt arrives before any transcript delta has been
    /// staged.
    TurnInterrupted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
    },
    TurnCompleted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        stop_reason: WireStopReason,
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        usage: Usage,
    },
    StatusChanged {
        status: WireLiveAdapterStatus,
    },
    Error {
        code: WireLiveAdapterErrorCode,
        message: String,
    },
    /// R5-9: scoped command rejection. Channel survives — distinct from
    /// terminal [`Self::Error`].
    CommandRejected {
        code: WireLiveAdapterErrorCode,
        message: String,
    },
}

impl From<LiveAdapterObservation> for WireLiveAdapterObservation {
    fn from(value: LiveAdapterObservation) -> Self {
        match value {
            LiveAdapterObservation::Ready => Self::Ready,
            LiveAdapterObservation::UserTranscriptFinal {
                provider_item_id,
                previous_item_id,
                content_index,
                text,
            } => Self::UserTranscriptFinal {
                provider_item_id,
                previous_item_id,
                content_index,
                text,
            },
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                delta_id,
                delta,
            } => Self::AssistantTextDelta {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                delta_id,
                delta,
            },
            LiveAdapterObservation::AssistantTranscriptDelta {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                delta_id,
                delta,
            } => Self::AssistantTranscriptDelta {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                delta_id,
                delta,
            },
            LiveAdapterObservation::AssistantAudioChunk {
                data,
                sample_rate_hz,
                channels,
                response_id,
                item_id,
                content_index,
            } => {
                use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
                Self::AssistantAudioChunk {
                    data: BASE64_STANDARD.encode(&data),
                    sample_rate_hz,
                    channels,
                    response_id,
                    item_id,
                    content_index,
                }
            }
            LiveAdapterObservation::AssistantTranscriptFinal {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                text,
                stop_reason,
                usage,
            } => Self::AssistantTranscriptFinal {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                text,
                stop_reason: WireStopReason::from(stop_reason),
                usage,
            },
            LiveAdapterObservation::AssistantTranscriptTruncated {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                text,
            } => Self::AssistantTranscriptTruncated {
                provider_item_id,
                previous_item_id,
                content_index,
                response_id,
                text,
            },
            LiveAdapterObservation::RealtimeTranscript { event } => {
                Self::RealtimeTranscript { event }
            }
            LiveAdapterObservation::ToolCallRequested {
                provider_call_id,
                tool_name,
                arguments,
            } => Self::ToolCallRequested {
                provider_call_id,
                tool_name,
                arguments,
            },
            LiveAdapterObservation::TurnInterrupted { response_id } => {
                Self::TurnInterrupted { response_id }
            }
            LiveAdapterObservation::TurnCompleted {
                response_id,
                stop_reason,
                usage,
            } => Self::TurnCompleted {
                response_id,
                stop_reason: WireStopReason::from(stop_reason),
                usage,
            },
            LiveAdapterObservation::StatusChanged { status } => Self::StatusChanged {
                status: status.into(),
            },
            LiveAdapterObservation::Error { code, message } => Self::Error {
                code: code.into(),
                message,
            },
            LiveAdapterObservation::CommandRejected { code, message } => Self::CommandRejected {
                code: code.into(),
                message,
            },
            // Core enum is `non_exhaustive`; debug-assert for new variants.
            _ => {
                debug_assert!(
                    false,
                    "WireLiveAdapterObservation::from saw an unmapped \
                     LiveAdapterObservation variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::TurnInterrupted { response_id: None }
            }
        }
    }
}

/// Bridge variant: the wire mirror does not duplicate
/// [`meerkat_core::live_adapter::LiveAdapterStatus`] inside `Error.code`
/// payloads, so the only `From<Wire... > -> Core` path that matters here is
/// for the `Error` / `CommandRejected` branch (clients echoing typed errors
/// back to the runtime). A full inverse is not required for the SDK
/// observation surface — observations flow adapter -> wire -> SDK only —
/// but `WireStopReason` and `WireUsage` already provide the inverse via
/// their dedicated wire types in `wire/session.rs` and the helper above.
///
/// (No `impl From<WireLiveAdapterObservation> for LiveAdapterObservation`
/// emitted: the wire-side observation is a downstream projection, never the
/// authority. Adding one would invite reverse-direction flows that bypass
/// the adapter contract; the round-trip *value* equality is asserted via
/// JSON in `wire_live_adapter_observation_round_trips_for_all_variants`.)
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn live_open_params_round_trip() {
        let v = LiveOpenParams {
            session_id: "session-1".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveOpenParams = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_send_input_params_audio_chunk_round_trip() {
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::Audio {
                data: "AQID".into(),
                sample_rate_hz: 24_000,
                channels: 1,
            },
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveSendInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_send_input_params_text_chunk_round_trip() {
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::Text {
                text: "hello".into(),
            },
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveSendInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_send_input_params_image_chunk_round_trip() {
        // T11: image variant must round-trip via the wire mirror.
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::Image {
                mime: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
            },
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["chunk"]["kind"], "image");
        assert_eq!(j["chunk"]["mime"], "image/png");
        let back: LiveSendInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_send_input_params_video_frame_chunk_round_trip() {
        // T11: video-frame variant must round-trip via the wire mirror.
        let v = LiveSendInputParams {
            channel_id: "live_1".into(),
            chunk: LiveInputChunkWire::VideoFrame {
                codec: "vp8".into(),
                data: "AAECAwQ=".into(),
                timestamp_ms: 1_234,
            },
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["chunk"]["kind"], "video_frame");
        assert_eq!(j["chunk"]["codec"], "vp8");
        assert_eq!(j["chunk"]["timestamp_ms"], 1_234);
        let back: LiveSendInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_truncate_params_round_trip() {
        let v = LiveTruncateParams {
            channel_id: "live_1".into(),
            item_id: "item_42".into(),
            content_index: 0,
            audio_played_ms: 1_234,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        let back: LiveTruncateParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    // CC5 — typed capabilities mirror.

    #[test]
    fn wire_live_channel_capabilities_round_trip_serde() {
        // Full typed-boolean matrix survives wire round-trip with field
        // names visible (no opaque `serde_json::Value` shroud).
        let v = WireLiveChannelCapabilities {
            audio_in: true,
            audio_out: true,
            text_in: true,
            text_out: true,
            image_in: false,
            video_in: false,
            transcript_supported: true,
            barge_in_supported: true,
            provider_native_resume: false,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        // Each typed boolean is reachable on the wire object — closes the
        // CC5 finding that SDK clients had to handcraft access.
        assert_eq!(j["audio_in"], true);
        assert_eq!(j["audio_out"], true);
        assert_eq!(j["text_in"], true);
        assert_eq!(j["text_out"], true);
        assert_eq!(j["image_in"], false);
        assert_eq!(j["video_in"], false);
        assert_eq!(j["transcript_supported"], true);
        assert_eq!(j["barge_in_supported"], true);
        assert_eq!(j["provider_native_resume"], false);
        let back: WireLiveChannelCapabilities =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn wire_live_channel_capabilities_round_trip_through_core() {
        // Core ↔ wire conversion is a value-preserving bijection.
        let core = LiveChannelCapabilities {
            audio_in: true,
            audio_out: true,
            text_in: true,
            text_out: true,
            image_in: true, // `gpt-realtime-2` shape.
            video_in: true, // Gemini Live shape.
            transcript_supported: true,
            barge_in_supported: true,
            provider_native_resume: true,
        };
        let wire: WireLiveChannelCapabilities = core.clone().into();
        let back: LiveChannelCapabilities = wire.into();
        assert_eq!(core, back);
    }

    #[test]
    fn wire_live_channel_capabilities_anticipates_future_modalities() {
        // T8 + CC5 acceptance: the typed matrix represents `gpt-realtime-2`
        // (image_in) and Gemini Live (video_in) without provider-specific
        // fields.
        let gpt_realtime_2 = WireLiveChannelCapabilities {
            audio_in: true,
            audio_out: true,
            text_in: true,
            text_out: true,
            image_in: true,
            video_in: false,
            transcript_supported: true,
            barge_in_supported: true,
            provider_native_resume: false,
        };
        let gemini_live = WireLiveChannelCapabilities {
            audio_in: true,
            audio_out: true,
            text_in: true,
            text_out: true,
            image_in: false,
            video_in: true,
            transcript_supported: true,
            barge_in_supported: true,
            provider_native_resume: false,
        };
        let g1 = serde_json::to_value(&gpt_realtime_2).expect("round-trip should succeed");
        let g2 = serde_json::to_value(&gemini_live).expect("round-trip should succeed");
        assert_eq!(g1["image_in"], true);
        assert_eq!(g1["video_in"], false);
        assert_eq!(g2["image_in"], false);
        assert_eq!(g2["video_in"], true);
    }

    // CC6 — typed continuity-mode mirror.

    #[test]
    fn wire_live_continuity_mode_payload_less_variants_round_trip() {
        for v in [
            WireLiveContinuityMode::Fresh,
            WireLiveContinuityMode::TranscriptOnly,
            WireLiveContinuityMode::Degraded,
        ] {
            let j = serde_json::to_value(&v).expect("round-trip should succeed");
            // Internally-tagged on `mode` (snake_case) — matches the core
            // enum's serde shape exactly.
            assert!(j.get("mode").is_some(), "missing `mode` discriminator");
            let back: WireLiveContinuityMode =
                serde_json::from_value(j).expect("round-trip should succeed");
            assert_eq!(v, back);
        }
    }

    #[test]
    fn wire_live_continuity_mode_provider_native_resume_round_trip() {
        let v = WireLiveContinuityMode::ProviderNativeResume {
            provider_session_id: "rtsess_abc123".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        // Tagged as `mode: "provider_native_resume"`, payload field
        // `provider_session_id` flat alongside the discriminator.
        assert_eq!(j["mode"], "provider_native_resume");
        assert_eq!(j["provider_session_id"], "rtsess_abc123");
        let back: WireLiveContinuityMode =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn wire_live_continuity_mode_byte_compatible_with_core() {
        // The wire mirror serializes byte-identical to the core enum so
        // existing clients keep working across the typed-shape transition.
        let core_resume = LiveContinuityMode::ProviderNativeResume {
            provider_session_id: "sess_xyz".into(),
        };
        let wire_resume: WireLiveContinuityMode = core_resume.clone().into();
        let core_json = serde_json::to_value(&core_resume).expect("round-trip should succeed");
        let wire_json = serde_json::to_value(&wire_resume).expect("round-trip should succeed");
        assert_eq!(core_json, wire_json);

        let core_transcript = LiveContinuityMode::TranscriptOnly;
        let wire_transcript: WireLiveContinuityMode = core_transcript.clone().into();
        assert_eq!(
            serde_json::to_value(&core_transcript).expect("round-trip should succeed"),
            serde_json::to_value(&wire_transcript).expect("round-trip should succeed"),
        );
    }

    #[test]
    fn wire_live_continuity_mode_round_trips_through_core() {
        for v in [
            WireLiveContinuityMode::Fresh,
            WireLiveContinuityMode::TranscriptOnly,
            WireLiveContinuityMode::Degraded,
            WireLiveContinuityMode::ProviderNativeResume {
                provider_session_id: "sess_back".into(),
            },
        ] {
            let core: LiveContinuityMode = v.clone().into();
            let back: WireLiveContinuityMode = core.into();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn live_open_result_typed_capabilities_and_continuity_round_trip() {
        // The `LiveOpenResult` fields are now typed at the wire boundary —
        // SDK codegen sees real shapes, not `serde_json::Value`.
        let v = LiveOpenResult {
            channel_id: "live_1".into(),
            transport: WireLiveTransportBootstrap::Websocket {
                url: "wss://example/live".into(),
                token: "tok".into(),
            },
            capabilities: WireLiveChannelCapabilities {
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
            continuity: WireLiveContinuityMode::TranscriptOnly,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        // Typed access at the JSON layer too.
        assert_eq!(j["capabilities"]["audio_in"], true);
        assert_eq!(j["capabilities"]["image_in"], false);
        assert_eq!(j["continuity"]["mode"], "transcript_only");
        let back: LiveOpenResult = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    // FIX-SDK-OBS — typed `LiveAdapterObservation` wire mirror.

    #[test]
    fn wire_live_adapter_observation_round_trips_for_all_variants() {
        // Each typed variant survives serde round-trip with its
        // discriminator and payload visible. Reads exercise the
        // R5-4 identity fields on `assistant_audio_chunk` and the R5-9
        // `command_rejected` typed error code.

        let cases: Vec<WireLiveAdapterObservation> = vec![
            WireLiveAdapterObservation::Ready,
            WireLiveAdapterObservation::UserTranscriptFinal {
                provider_item_id: Some("item_user_1".into()),
                previous_item_id: None,
                content_index: Some(0),
                text: "hello".into(),
            },
            WireLiveAdapterObservation::AssistantTextDelta {
                provider_item_id: Some("item_a".into()),
                previous_item_id: None,
                content_index: Some(0),
                response_id: Some("resp_1".into()),
                delta_id: Some("delta_1".into()),
                delta: "hi".into(),
            },
            WireLiveAdapterObservation::AssistantTranscriptDelta {
                provider_item_id: Some("item_b".into()),
                previous_item_id: None,
                content_index: Some(0),
                response_id: Some("resp_1".into()),
                delta_id: Some("delta_2".into()),
                delta: "spoken".into(),
            },
            // R5-4: identity fields populated on the audio chunk so the
            // wire mirror exposes them to SDK clients driving
            // `live/truncate`.
            WireLiveAdapterObservation::AssistantAudioChunk {
                data: "AQID".into(),
                sample_rate_hz: 24_000,
                channels: 1,
                response_id: Some("resp_audio".into()),
                item_id: Some("item_audio".into()),
                content_index: Some(0),
            },
            WireLiveAdapterObservation::AssistantTranscriptFinal {
                provider_item_id: "item_final".into(),
                previous_item_id: None,
                content_index: Some(0),
                response_id: Some("resp_final".into()),
                text: "all done".into(),
                stop_reason: WireStopReason::EndTurn,
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 7,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
            WireLiveAdapterObservation::AssistantTranscriptTruncated {
                provider_item_id: Some("item_trunc".into()),
                previous_item_id: None,
                content_index: Some(0),
                response_id: Some("resp_trunc".into()),
                text: Some("partial".into()),
            },
            WireLiveAdapterObservation::ToolCallRequested {
                provider_call_id: "call_1".into(),
                tool_name: "lookup".into(),
                arguments: serde_json::json!({"q": "weather"}),
            },
            WireLiveAdapterObservation::TurnInterrupted {
                response_id: Some("resp_interrupt".into()),
            },
            WireLiveAdapterObservation::TurnCompleted {
                response_id: Some("resp_done".into()),
                stop_reason: WireStopReason::EndTurn,
                usage: Usage {
                    input_tokens: 12,
                    output_tokens: 34,
                    cache_creation_tokens: Some(1),
                    cache_read_tokens: Some(2),
                },
            },
            WireLiveAdapterObservation::StatusChanged {
                status: WireLiveAdapterStatus::Degraded {
                    reason: WireLiveDegradationReason::RateLimited,
                },
            },
            WireLiveAdapterObservation::Error {
                code: WireLiveAdapterErrorCode::ConnectionLost,
                message: "transport gone".into(),
            },
            // R5-9: typed `CommandRejected` is now a first-class wire variant.
            WireLiveAdapterObservation::CommandRejected {
                code: WireLiveAdapterErrorCode::ConfigRejected {
                    reason: "image_input_not_implemented".into(),
                },
                message: "adapter rejected image".into(),
            },
        ];

        for case in cases {
            let j = serde_json::to_value(&case).expect("round-trip should succeed");
            assert!(
                j.get("observation").is_some(),
                "missing `observation` discriminator on {case:?}"
            );
            let back: WireLiveAdapterObservation =
                serde_json::from_value(j).expect("round-trip should succeed");
            assert_eq!(case, back);
        }
    }

    #[test]
    fn wire_live_adapter_observation_assistant_audio_chunk_identity_fields_visible() {
        // The R5-4 identity fields (`response_id`, `item_id`,
        // `content_index`) appear flat on the JSON object so SDK
        // consumers can drive `live/truncate` typed.
        let v = WireLiveAdapterObservation::AssistantAudioChunk {
            data: "AQID".into(),
            sample_rate_hz: 24_000,
            channels: 1,
            response_id: Some("resp_audio".into()),
            item_id: Some("item_audio".into()),
            content_index: Some(2),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["observation"], "assistant_audio_chunk");
        assert_eq!(j["item_id"], "item_audio");
        assert_eq!(j["response_id"], "resp_audio");
        assert_eq!(j["content_index"], 2);
        assert_eq!(j["sample_rate_hz"], 24_000);
        assert_eq!(j["channels"], 1);
    }

    #[test]
    fn wire_live_adapter_observation_command_rejected_visible_as_typed_variant() {
        let v = WireLiveAdapterObservation::CommandRejected {
            code: WireLiveAdapterErrorCode::ConfigRejected {
                reason: "video_frame_input_not_implemented".into(),
            },
            message: "rejected".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["observation"], "command_rejected");
        assert_eq!(j["code"]["code"], "config_rejected");
        assert_eq!(j["code"]["reason"], "video_frame_input_not_implemented");
    }

    #[test]
    fn wire_live_adapter_observation_byte_compatible_with_core_for_audio_chunk() {
        // Wire mirror serializes byte-identical to the core enum for the
        // R5-4 identity-bearing variant. Catches drift between the two
        // shapes the moment a field is added to one and forgotten on the
        // other.
        let core = LiveAdapterObservation::AssistantAudioChunk {
            data: vec![1, 2, 3],
            sample_rate_hz: 24_000,
            channels: 1,
            response_id: Some("resp_audio".into()),
            item_id: Some("item_audio".into()),
            content_index: Some(2),
        };
        let wire: WireLiveAdapterObservation = core.clone().into();
        let core_json = serde_json::to_value(&core).expect("round-trip should succeed");
        let wire_json = serde_json::to_value(&wire).expect("round-trip should succeed");
        assert_eq!(core_json, wire_json);
    }

    #[test]
    fn wire_live_adapter_observation_byte_compatible_with_core_for_command_rejected() {
        let core = LiveAdapterObservation::CommandRejected {
            code: LiveAdapterErrorCode::ConfigRejected {
                reason: "image_input_not_implemented".into(),
            },
            message: "rejected".into(),
        };
        let wire: WireLiveAdapterObservation = core.clone().into();
        let core_json = serde_json::to_value(&core).expect("round-trip should succeed");
        let wire_json = serde_json::to_value(&wire).expect("round-trip should succeed");
        assert_eq!(core_json, wire_json);
    }

    // G8 (P2) — typed `LiveTransportBootstrap` wire mirror.

    #[test]
    fn wire_live_transport_bootstrap_websocket_round_trip() {
        let v = WireLiveTransportBootstrap::Websocket {
            url: "wss://example/live".into(),
            token: "tok_abc".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        // Internally-tagged on `transport` (snake_case), payload fields
        // flat alongside the discriminator — matches the core enum's serde
        // shape exactly.
        assert_eq!(j["transport"], "websocket");
        assert_eq!(j["url"], "wss://example/live");
        assert_eq!(j["token"], "tok_abc");
        let back: WireLiveTransportBootstrap =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn wire_live_transport_bootstrap_byte_compatible_with_core() {
        // Wire mirror serializes byte-identical to the core enum. Catches
        // drift the moment a field is added to one and forgotten on the
        // other.
        let core = LiveTransportBootstrap::Websocket {
            url: "wss://example/live".into(),
            token: "tok_xyz".into(),
        };
        let wire: WireLiveTransportBootstrap = core.clone().into();
        let core_json = serde_json::to_value(&core).expect("round-trip should succeed");
        let wire_json = serde_json::to_value(&wire).expect("round-trip should succeed");
        assert_eq!(core_json, wire_json);
    }

    #[test]
    fn wire_live_transport_bootstrap_round_trips_through_core() {
        let v = WireLiveTransportBootstrap::Websocket {
            url: "wss://example/live".into(),
            token: "tok_back".into(),
        };
        let core: LiveTransportBootstrap = v.clone().into();
        let back: WireLiveTransportBootstrap = core.into();
        assert_eq!(v, back);
    }

    // G9 (P2) — typed `LiveResponseModality` wire mirror + commit-input
    // params shape.

    #[test]
    fn wire_live_response_modality_payload_less_variants_round_trip() {
        for v in [
            WireLiveResponseModality::Audio,
            WireLiveResponseModality::Text,
        ] {
            let j = serde_json::to_value(v).expect("round-trip should succeed");
            assert!(
                j.get("modality").is_some(),
                "missing `modality` discriminator"
            );
            let back: WireLiveResponseModality =
                serde_json::from_value(j).expect("round-trip should succeed");
            assert_eq!(v, back);
        }
    }

    #[test]
    fn wire_live_response_modality_byte_compatible_with_core() {
        for core in [LiveResponseModality::Audio, LiveResponseModality::Text] {
            let wire: WireLiveResponseModality = core.into();
            let core_json = serde_json::to_value(core).expect("round-trip should succeed");
            let wire_json = serde_json::to_value(wire).expect("round-trip should succeed");
            assert_eq!(core_json, wire_json);
        }
    }

    #[test]
    fn live_commit_input_params_default_modality_round_trip() {
        // Omitted `response_modality` keeps the channel default — the JSON
        // payload elides the field entirely (skip_serializing_if).
        let v = LiveCommitInputParams {
            channel_id: "live_1".into(),
            response_modality: None,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["channel_id"], "live_1");
        assert!(
            j.get("response_modality").is_none(),
            "default-modality params must elide the field"
        );
        let back: LiveCommitInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_commit_input_params_text_modality_round_trip() {
        let v = LiveCommitInputParams {
            channel_id: "live_1".into(),
            response_modality: Some(WireLiveResponseModality::Text),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["channel_id"], "live_1");
        assert_eq!(j["response_modality"]["modality"], "text");
        let back: LiveCommitInputParams =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }
}
