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

use meerkat_core::Provider;
use meerkat_core::live_adapter::{
    LiveAdapterErrorCode, LiveAdapterObservation, LiveAdapterStatus, LiveChannelCapabilities,
    LiveConfigRejectionReason, LiveContinuityMode, LiveDegradationReason, LiveResponseModality,
    LiveTransportBootstrap,
};
use meerkat_core::realtime_transcript::RealtimeTranscriptEvent;

use crate::wire::realtime::RealtimeTurningMode;
use crate::wire::session::WireStopReason;

/// Wire-safe projection of [`meerkat_core::Provider`].
///
/// The core `Provider` enum uses `#[serde(rename_all = "snake_case")]` which
/// transforms `OpenAI` to `"open_a_i"` on the wire -- not the conventional
/// `"openai"`. `WireProvider` pins the correct wire names with explicit
/// `#[serde(rename)]` on each variant so SDK consumers see `"openai"`,
/// `"anthropic"`, `"gemini"`, etc.
///
/// Includes an `Unknown { debug: String }` variant for future-proofing per
/// the wire-mirror dogma used throughout this module.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum WireProvider {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "self_hosted")]
    SelfHosted,
    #[serde(rename = "other")]
    Other,
    /// Fail-loud sentinel for future core variants. When a new
    /// `Provider` variant is added, add an explicit arm in the `From`
    /// impl rather than letting it fall through to `Unknown`.
    ///
    /// Unlike other wire-mirror `Unknown` variants, this is a unit
    /// variant because `WireProvider` serializes as a flat string value
    /// (not an internally-tagged object). The debug payload from the
    /// source variant is captured in the `WireConversionError::Provider`
    /// error on the reverse path (and logged server-side in the forward
    /// `From<Provider>` impl via `debug_assert!`).
    #[serde(rename = "unknown")]
    Unknown,
}

impl From<Provider> for WireProvider {
    fn from(value: Provider) -> Self {
        match value {
            Provider::Anthropic => Self::Anthropic,
            Provider::OpenAI => Self::OpenAi,
            Provider::Gemini => Self::Gemini,
            Provider::SelfHosted => Self::SelfHosted,
            Provider::Other => Self::Other,
            // Core `Provider` is not `#[non_exhaustive]` today, but the
            // wildcard arm future-proofs the wire boundary.
            #[allow(unreachable_patterns)]
            other => {
                debug_assert!(
                    false,
                    "WireProvider::from saw an unmapped Provider variant: {other:?}; \
                     add an explicit arm in meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown
            }
        }
    }
}

impl TryFrom<WireProvider> for Provider {
    type Error = WireConversionError;

    fn try_from(value: WireProvider) -> Result<Self, Self::Error> {
        match value {
            WireProvider::Anthropic => Ok(Self::Anthropic),
            WireProvider::OpenAi => Ok(Self::OpenAI),
            WireProvider::Gemini => Ok(Self::Gemini),
            WireProvider::SelfHosted => Ok(Self::SelfHosted),
            WireProvider::Other => Ok(Self::Other),
            WireProvider::Unknown => Err(WireConversionError::Provider {
                debug: "WireProvider::Unknown".to_string(),
            }),
        }
    }
}

/// Request payload for `live/open`.
///
/// R3-1 (P1): `turning_mode` lets callers pick between the provider-managed
/// (server-VAD) flow and the explicit-commit flow. The G9 typed text-only
/// `live/commit_input { response_modality: text }` path requires
/// `ExplicitCommit` because the OpenAI realtime API rejects
/// `input_audio_buffer.commit` unless the session was opened with explicit
/// commit semantics (server VAD owns commits otherwise). Pre-R3-1 the
/// handler hard-coded `ProviderManaged`, which made the typed text-only
/// path unreachable on a public channel — calling it killed the channel
/// instead of producing sideband text.
///
/// Default (`None`) preserves the prior wire shape: callers that omit the
/// field get `ProviderManaged`, matching the legacy behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveOpenParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turning_mode: Option<RealtimeTurningMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<LiveOpenTransport>,
}

/// Transport requested by `live/open`.
///
/// Missing means "server default": prefer WebSocket when configured, otherwise
/// WebRTC when that is the only configured live transport. This preserves the
/// pre-WebRTC `live/open` shape while keeping the new branch typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveOpenTransport {
    Websocket,
    Webrtc,
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
    /// WebRTC bootstrap: client calls `answer_method` with
    /// [`LiveWebrtcAnswerParams`] using this single-use token. `http_url`
    /// is optional convenience signaling; JSON-RPC is canonical.
    Webrtc {
        token: String,
        answer_method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_url: Option<String>,
    },
    /// R3-6 (P2): explicit fail-loud variant for unknown core variants.
    ///
    /// The core [`LiveTransportBootstrap`] enum is `#[non_exhaustive]`. When
    /// a future variant (e.g. WebRTC reintroduction per Round-4 T4) lands
    /// without an explicit arm in the wire `From` impl, the conversion
    /// surfaces as `Unknown { debug }` rather than silently coercing into
    /// `Websocket { url: "", token: "" }`. SDK consumers route on the
    /// `transport: "unknown"` discriminator and treat it as an unsupported
    /// transport — never as a usable websocket. The `debug` payload carries
    /// the `{:?}` projection of the source variant so server logs name the
    /// real shape that needs to be promoted.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor, not
    /// the destination.**
    Unknown { debug: String },
}

impl From<LiveTransportBootstrap> for WireLiveTransportBootstrap {
    fn from(value: LiveTransportBootstrap) -> Self {
        match value {
            LiveTransportBootstrap::Websocket { url, token } => Self::Websocket { url, token },
            LiveTransportBootstrap::Webrtc {
                token,
                answer_method,
                http_url,
            } => Self::Webrtc {
                token,
                answer_method,
                http_url,
            },
            // Core enum is `#[non_exhaustive]`. R3-6 (P2): surface unknown
            // variants explicitly via `Unknown { debug }` rather than
            // silently coercing to a bogus `Websocket { url: "", token: "" }`.
            // The `debug` payload preserves the source variant name for
            // server-side observability. When a new core variant lands,
            // add an explicit arm above this comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveTransportBootstrap::from saw an unmapped \
                     LiveTransportBootstrap variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

// Re-export from the shared wire error module for backwards compatibility.
// Callers that previously imported `crate::wire::live::WireConversionError`
// continue to compile; new code should import from `crate::wire::WireConversionError`.
pub use crate::wire::error::WireConversionError;

impl TryFrom<WireLiveTransportBootstrap> for LiveTransportBootstrap {
    type Error = WireConversionError;

    fn try_from(value: WireLiveTransportBootstrap) -> Result<Self, Self::Error> {
        // Wire enum is owned by this crate so even with `#[non_exhaustive]`
        // (which only constrains matches outside the defining crate) the
        // compiler enforces exhaustive coverage here. Adding a new wire
        // variant therefore forces a new arm in this match — no silent
        // fallthrough.
        match value {
            WireLiveTransportBootstrap::Websocket { url, token } => {
                Ok(Self::Websocket { url, token })
            }
            WireLiveTransportBootstrap::Webrtc {
                token,
                answer_method,
                http_url,
            } => Ok(Self::Webrtc {
                token,
                answer_method,
                http_url,
            }),
            WireLiveTransportBootstrap::Unknown { debug } => {
                Err(WireConversionError::Transport { debug })
            }
        }
    }
}

/// Request payload for `live/webrtc/answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveWebrtcAnswerParams {
    pub channel_id: String,
    pub token: String,
    pub offer_sdp: String,
}

/// Response payload for `live/webrtc/answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveWebrtcAnswerResult {
    pub answer_sdp: String,
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
    /// R5-3 (P3): explicit fail-loud variant for unknown core variants.
    ///
    /// The core [`LiveContinuityMode`] enum is `#[non_exhaustive]`. When a
    /// future variant lands without an explicit arm in the wire `From`
    /// impl, the conversion surfaces as `Unknown { debug }` rather than
    /// silently coercing into `Fresh` — a "plausible lie" that would tell
    /// SDK consumers a brand-new live channel exists when in reality the
    /// server emitted a continuity mode the client does not yet
    /// understand. SDK consumers route on the `mode: "unknown"`
    /// discriminator and treat it as unrecognized — never as `Fresh`.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown { debug: String },
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
            // Core enum is `#[non_exhaustive]`. R5-3 (P3): surface unknown
            // variants explicitly via `Unknown { debug }` rather than
            // silently coercing to `Fresh` (the previous fail-open default
            // — a plausible lie that would mask a real continuity-mode
            // change as a brand-new channel). When a new core variant
            // lands, add an explicit arm above this comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveContinuityMode::from saw an unmapped \
                     LiveContinuityMode variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

impl TryFrom<WireLiveContinuityMode> for LiveContinuityMode {
    type Error = WireConversionError;

    fn try_from(value: WireLiveContinuityMode) -> Result<Self, Self::Error> {
        // No wildcard arm: `WireLiveContinuityMode` is owned by this crate
        // so even with `#[non_exhaustive]` (which only constrains matches
        // outside the defining crate) the compiler enforces exhaustive
        // coverage here. Adding a new wire variant therefore forces a new
        // arm in this match — no silent fallthrough.
        match value {
            WireLiveContinuityMode::Fresh => Ok(Self::Fresh),
            WireLiveContinuityMode::TranscriptOnly => Ok(Self::TranscriptOnly),
            WireLiveContinuityMode::Degraded => Ok(Self::Degraded),
            WireLiveContinuityMode::ProviderNativeResume {
                provider_session_id,
            } => Ok(Self::ProviderNativeResume {
                provider_session_id,
            }),
            WireLiveContinuityMode::Unknown { debug } => {
                Err(WireConversionError::Continuity { debug })
            }
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
///
/// R5-3 (P3): no longer `Copy` because `Unknown { debug: String }` carries
/// an owned payload. The enum is small and conversions across the boundary
/// move-or-clone, so the loss of `Copy` is not material at call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "modality", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveResponseModality {
    /// Spoken-audio plus the audio-derived transcript.
    Audio,
    /// Display text only — no audio output, no transcript.
    Text,
    /// R5-3 (P3): explicit fail-loud variant for unknown core variants.
    ///
    /// The core [`LiveResponseModality`] enum is `#[non_exhaustive]`. When
    /// a future modality (e.g. structured output, image) lands without an
    /// explicit arm in the wire `From` impl, the conversion surfaces as
    /// `Unknown { debug }` rather than silently coercing to `Audio` — a
    /// "plausible lie" that would route a future text/structured response
    /// through the audio playback path and drop content. SDK consumers
    /// route on the `modality: "unknown"` discriminator and treat it as an
    /// unsupported modality — never as `Audio`.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown { debug: String },
}

impl From<LiveResponseModality> for WireLiveResponseModality {
    fn from(value: LiveResponseModality) -> Self {
        match value {
            LiveResponseModality::Audio => Self::Audio,
            LiveResponseModality::Text => Self::Text,
            // Core enum is `#[non_exhaustive]`. R5-3 (P3): surface unknown
            // variants explicitly via `Unknown { debug }` rather than
            // silently coercing to `Audio` (the previous fail-open default
            // — a plausible lie that would route a future text/structured
            // response through the audio playback path). When a new core
            // variant lands, add an explicit arm above this comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveResponseModality::from saw an unmapped \
                     LiveResponseModality variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

impl TryFrom<WireLiveResponseModality> for LiveResponseModality {
    type Error = WireConversionError;

    fn try_from(value: WireLiveResponseModality) -> Result<Self, Self::Error> {
        // No wildcard arm: `WireLiveResponseModality` is owned by this
        // crate so even with `#[non_exhaustive]` the compiler enforces
        // exhaustive coverage here.
        match value {
            WireLiveResponseModality::Audio => Ok(Self::Audio),
            WireLiveResponseModality::Text => Ok(Self::Text),
            WireLiveResponseModality::Unknown { debug } => {
                Err(WireConversionError::ResponseModality { debug })
            }
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

/// Response payload for `live/status`.
///
/// R6-3 (P2): `status` is now the typed [`WireLiveAdapterStatus`] mirror
/// (with R5-3's `Unknown { debug }` floor) instead of the core
/// [`LiveAdapterStatus`] under a `schemars(with = "serde_json::Value")`
/// shroud. SDK codegen emits the typed discriminated union (TS) / typed
/// dict union (Python) for `live/status` instead of `unknown` / `Any`.
/// The runtime handler converts core → wire at the boundary; clients
/// that fully understood the previous JSON shape see byte-identical
/// payloads (the wire mirror serializes byte-compatible with the core
/// enum — see `wire_live_adapter_status_byte_compatible_with_core`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveStatusResult {
    pub channel_id: String,
    pub status: WireLiveAdapterStatus,
}

/// Status of a `live/refresh` request relative to the adapter pump.
///
/// R4-5 (P3): the refresh path is asynchronous — `LiveAdapterHost::send_command`
/// returns when the command has been queued on the adapter's mpsc channel,
/// not when the adapter pump has applied the resulting `session.update`. The
/// realtime stream is the source of truth for the actual refresh outcome
/// (failures surface as `LiveAdapterObservation::Error`).
///
/// Today every refresh path is `Queued`. The enum is `#[non_exhaustive]` so
/// a future revision can add `AppliedSync` (e.g. when a oneshot ack from the
/// adapter pump back through the command channel lands, or when a refresh
/// is detected as a no-op against the currently-applied snapshot) without
/// breaking the wire shape. SDK consumers route on the string value and
/// treat unknown values as "outcome unknown — observe the realtime stream".
///
/// Serializes as a plain string (no envelope) so [`LiveRefreshResult`] can
/// place this typed status alongside the back-compat `refresh_enqueued`
/// boolean as ordinary sibling fields, which keeps SDK codegen on the
/// simple-struct path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveRefreshStatus {
    /// The host has accepted the refresh command onto the adapter's mpsc
    /// queue. The adapter pump applies the `session.update` asynchronously;
    /// callers that need the actual outcome must observe the adapter's
    /// realtime stream.
    Queued,
}

/// Response payload for `live/refresh`.
///
/// R4-5 (P3): replaces the previous untyped `{"refresh_enqueued": true}`
/// JSON blob. The boolean `refresh_enqueued` field is preserved for back-
/// compat (legacy clients that pattern-match on it stay on the green path)
/// alongside the typed `status` discriminator. New code should route on
/// `status`.
///
/// See [`LiveRefreshStatus`] for the variant set and the contract on
/// asynchronous adapter-pump application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LiveRefreshResult {
    /// Typed refresh status. Today: always [`LiveRefreshStatus::Queued`].
    pub status: LiveRefreshStatus,
    /// Back-compat mirror of the legacy untyped reply field. Always `true`
    /// when paired with `status: queued`. New code should route on `status`.
    pub refresh_enqueued: bool,
}

impl LiveRefreshResult {
    /// Construct a `Queued` result — the only outcome the host's
    /// `send_command` path produces today.
    pub fn queued() -> Self {
        Self {
            status: LiveRefreshStatus::Queued,
            refresh_enqueued: true,
        }
    }
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
    Unknown { debug: String },
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
            other => {
                debug_assert!(
                    false,
                    "WireLiveDegradationReason::from saw an unmapped \
                     LiveDegradationReason variant: {other:?}; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

impl TryFrom<WireLiveDegradationReason> for LiveDegradationReason {
    type Error = WireConversionError;

    fn try_from(value: WireLiveDegradationReason) -> Result<Self, Self::Error> {
        match value {
            WireLiveDegradationReason::RateLimited => Ok(Self::RateLimited),
            WireLiveDegradationReason::ProviderThrottled => Ok(Self::ProviderThrottled),
            WireLiveDegradationReason::NetworkUnstable => Ok(Self::NetworkUnstable),
            WireLiveDegradationReason::Other { detail } => Ok(Self::Other {
                detail: detail.into(),
            }),
            WireLiveDegradationReason::Unknown { debug } => {
                Err(WireConversionError::DegradationReason { debug })
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
    Degraded {
        reason: WireLiveDegradationReason,
    },
    Closing,
    Closed,
    /// R5-3 (P3): explicit fail-loud variant for unknown core variants.
    ///
    /// The core [`LiveAdapterStatus`] enum is `#[non_exhaustive]`. When a
    /// future status lands without an explicit arm in the wire `From`
    /// impl, the conversion surfaces as `Unknown { debug }` rather than
    /// silently coercing to `Closed` — a "plausible lie" that would tell
    /// SDK consumers a healthy channel was torn down. SDK consumers route
    /// on the `status: "unknown"` discriminator and treat it as an
    /// unrecognized status — never as `Closed`.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown {
        debug: String,
    },
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
            // Core enum is `#[non_exhaustive]`. R5-3 (P3): surface unknown
            // variants explicitly via `Unknown { debug }` rather than
            // silently coercing to `Closed` (the previous fail-open default
            // — a plausible lie that would tell consumers a healthy
            // channel was torn down). When a new core variant lands, add
            // an explicit arm above this comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveAdapterStatus::from saw an unmapped \
                     LiveAdapterStatus variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

// (No `impl From/TryFrom<WireLiveAdapterStatus> for LiveAdapterStatus`
// emitted: like `WireLiveAdapterObservation`, the wire-side adapter status
// is a downstream projection — never authority. The `Unknown` arm exists
// so future core variants surface explicitly on the forward path; the
// `WireConversionError::Status` variant is reserved for a future
// caller that needs to construct a typed inverse and will materialize the
// reverse impl alongside the dependent `From<WireLiveDegradationReason>`
// addition. No production code calls the inverse today.)

/// Wire mirror of [`meerkat_core::live_adapter::LiveAdapterErrorCode`].
///
/// Internally-tagged on `code` (snake_case). SDK consumers route on `code`
/// to distinguish payload-less variants from typed payload variants
/// (`config_rejected { reason }`, `other { raw }`). FIX-SDK-OBS: makes the
/// R5-9 `CommandRejected` observation's typed code visible at the SDK
/// boundary instead of a free-form blob.
///
/// R5-2 (P2 dogma): `config_rejected.reason` is now a typed
/// [`WireLiveConfigRejectionReason`] mirror so SDK consumers route on the
/// variant rather than parsing English from the previous free-form
/// `String`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveAdapterErrorCode {
    ConnectionFailed,
    ConnectionLost,
    ConfigRejected {
        reason: WireLiveConfigRejectionReason,
    },
    ProviderError,
    AuthenticationFailed,
    InternalError,
    Other {
        raw: String,
    },
    /// R5-3 (P3): explicit fail-loud variant for unknown core variants.
    ///
    /// The core [`LiveAdapterErrorCode`] enum is `#[non_exhaustive]`. When
    /// a future error code lands without an explicit arm in the wire
    /// `From` impl, the conversion surfaces as `Unknown { debug }` rather
    /// than silently coercing to `InternalError` — a "plausible lie" that
    /// would attribute every future-variant failure to an internal-server
    /// bug and mask real provider/transport classification. SDK consumers
    /// route on the `code: "unknown"` discriminator and treat it as an
    /// unrecognized error class — never as `InternalError`.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown {
        debug: String,
    },
}

/// Wire mirror of
/// [`meerkat_core::live_adapter::LiveConfigRejectionReason`]. R5-2 (P2
/// dogma): pins the typed semantic-routing variants on the wire so SDKs
/// can distinguish a runtime-side identity swap from an adapter-side
/// input-modality rejection without string parsing.
///
/// R6-5 (P3 dogma): closes the last typed-route-as-detail-string gap. The
/// previous wildcard fallback collapsed future core variants into
/// `Other { detail: "unknown_live_config_rejection_reason" }`, which SDK
/// consumers had to pattern-match on the English `detail` to route — the
/// exact "typed route becomes detail string" antipattern R3-6 + R5-3
/// closed for transport / observation / continuity / modality / status /
/// error_code. Future core variants now surface as `Unknown { debug }`
/// with the `{:?}` projection of the source variant preserved for
/// server-side observability.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireLiveConfigRejectionReason {
    ChannelIdentitySwap {
        from_model: String,
        from_provider: WireProvider,
        to_model: String,
        to_provider: WireProvider,
    },
    NonRealtimeResolution {
        detail: String,
    },
    ImageInputNotImplemented,
    VideoFrameInputNotImplemented,
    UnsupportedInputChunkVariant,
    RefreshModelSwap {
        from_model: String,
        to_model: String,
    },
    RefreshProviderSwap {
        from_provider: String,
        to_provider: String,
    },
    RefreshAudioConfigMismatch {
        detail: String,
    },
    /// R6-4 (P2): typed mirror of
    /// [`LiveConfigRejectionReason::AudioInputFormatMismatch`]. The
    /// bound provider session has a fixed input audio format (OpenAI
    /// Realtime: PCM 24 kHz mono); a `live/send_input` chunk that
    /// declares a divergent `sample_rate_hz` / `channels` is rejected
    /// at the adapter boundary BEFORE bytes hit the provider buffer.
    /// SDK consumers route on this typed discriminator to surface a
    /// precise format-mismatch error rather than parsing English from
    /// `Other.detail`.
    AudioInputFormatMismatch {
        expected_sample_rate_hz: u32,
        expected_channels: u16,
        actual_sample_rate_hz: u32,
        actual_channels: u16,
    },
    Other {
        detail: String,
    },
    /// R6-5 (P3 dogma): explicit fail-loud variant for unknown core
    /// variants.
    ///
    /// The core [`LiveConfigRejectionReason`] enum is `#[non_exhaustive]`.
    /// When a future variant lands without an explicit arm in the wire
    /// `From` impl, the conversion surfaces as `Unknown { debug }` rather
    /// than silently coercing into `Other { detail:
    /// "unknown_live_config_rejection_reason" }` — a "typed route becomes
    /// detail string" antipattern that forces SDK consumers to parse
    /// English from `detail` to recover the missing route. SDK consumers
    /// route on the `kind: "unknown"` discriminator and treat it as an
    /// unrecognized rejection reason — never as `Other`. The `debug`
    /// payload carries the `{:?}` projection of the source variant so
    /// server logs name the real shape that needs to be promoted.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown {
        debug: String,
    },
}

impl From<LiveConfigRejectionReason> for WireLiveConfigRejectionReason {
    fn from(value: LiveConfigRejectionReason) -> Self {
        match value {
            LiveConfigRejectionReason::ChannelIdentitySwap {
                from_model,
                from_provider,
                to_model,
                to_provider,
            } => Self::ChannelIdentitySwap {
                from_model,
                from_provider: from_provider.into(),
                to_model,
                to_provider: to_provider.into(),
            },
            LiveConfigRejectionReason::NonRealtimeResolution { detail } => {
                Self::NonRealtimeResolution { detail }
            }
            LiveConfigRejectionReason::ImageInputNotImplemented => Self::ImageInputNotImplemented,
            LiveConfigRejectionReason::VideoFrameInputNotImplemented => {
                Self::VideoFrameInputNotImplemented
            }
            LiveConfigRejectionReason::UnsupportedInputChunkVariant => {
                Self::UnsupportedInputChunkVariant
            }
            LiveConfigRejectionReason::RefreshModelSwap {
                from_model,
                to_model,
            } => Self::RefreshModelSwap {
                from_model,
                to_model,
            },
            LiveConfigRejectionReason::RefreshProviderSwap {
                from_provider,
                to_provider,
            } => Self::RefreshProviderSwap {
                from_provider,
                to_provider,
            },
            LiveConfigRejectionReason::RefreshAudioConfigMismatch { detail } => {
                Self::RefreshAudioConfigMismatch { detail }
            }
            LiveConfigRejectionReason::AudioInputFormatMismatch {
                expected_sample_rate_hz,
                expected_channels,
                actual_sample_rate_hz,
                actual_channels,
            } => Self::AudioInputFormatMismatch {
                expected_sample_rate_hz,
                expected_channels,
                actual_sample_rate_hz,
                actual_channels,
            },
            LiveConfigRejectionReason::Other { detail } => Self::Other { detail },
            // Core enum is `#[non_exhaustive]`. R6-5 (P3 dogma): surface
            // unknown variants explicitly via `Unknown { debug }` rather
            // than silently coercing to `Other { detail:
            // "unknown_live_config_rejection_reason" }` (the previous
            // fail-open default — a "typed route becomes detail string"
            // antipattern that forced SDK consumers to parse English from
            // `detail` to recover the missing route). When a new core
            // variant lands, add an explicit arm above this comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveConfigRejectionReason::from saw an unmapped \
                     LiveConfigRejectionReason variant; add an explicit arm \
                     in meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

impl TryFrom<WireLiveConfigRejectionReason> for LiveConfigRejectionReason {
    type Error = WireConversionError;

    fn try_from(value: WireLiveConfigRejectionReason) -> Result<Self, Self::Error> {
        // No wildcard arm: `WireLiveConfigRejectionReason` is owned by this
        // crate so even with `#[non_exhaustive]` (which only constrains
        // matches outside the defining crate) the compiler enforces
        // exhaustive coverage here. Adding a new wire variant therefore
        // forces a new arm in this match — no silent fallthrough.
        match value {
            WireLiveConfigRejectionReason::ChannelIdentitySwap {
                from_model,
                from_provider,
                to_model,
                to_provider,
            } => Ok(Self::ChannelIdentitySwap {
                from_model,
                from_provider: from_provider.try_into()?,
                to_model,
                to_provider: to_provider.try_into()?,
            }),
            WireLiveConfigRejectionReason::NonRealtimeResolution { detail } => {
                Ok(Self::NonRealtimeResolution { detail })
            }
            WireLiveConfigRejectionReason::ImageInputNotImplemented => {
                Ok(Self::ImageInputNotImplemented)
            }
            WireLiveConfigRejectionReason::VideoFrameInputNotImplemented => {
                Ok(Self::VideoFrameInputNotImplemented)
            }
            WireLiveConfigRejectionReason::UnsupportedInputChunkVariant => {
                Ok(Self::UnsupportedInputChunkVariant)
            }
            WireLiveConfigRejectionReason::RefreshModelSwap {
                from_model,
                to_model,
            } => Ok(Self::RefreshModelSwap {
                from_model,
                to_model,
            }),
            WireLiveConfigRejectionReason::RefreshProviderSwap {
                from_provider,
                to_provider,
            } => Ok(Self::RefreshProviderSwap {
                from_provider,
                to_provider,
            }),
            WireLiveConfigRejectionReason::RefreshAudioConfigMismatch { detail } => {
                Ok(Self::RefreshAudioConfigMismatch { detail })
            }
            WireLiveConfigRejectionReason::AudioInputFormatMismatch {
                expected_sample_rate_hz,
                expected_channels,
                actual_sample_rate_hz,
                actual_channels,
            } => Ok(Self::AudioInputFormatMismatch {
                expected_sample_rate_hz,
                expected_channels,
                actual_sample_rate_hz,
                actual_channels,
            }),
            WireLiveConfigRejectionReason::Other { detail } => Ok(Self::Other { detail }),
            WireLiveConfigRejectionReason::Unknown { debug } => {
                Err(WireConversionError::ConfigRejectionReason { debug })
            }
        }
    }
}

impl From<LiveAdapterErrorCode> for WireLiveAdapterErrorCode {
    fn from(value: LiveAdapterErrorCode) -> Self {
        match value {
            LiveAdapterErrorCode::ConnectionFailed => Self::ConnectionFailed,
            LiveAdapterErrorCode::ConnectionLost => Self::ConnectionLost,
            LiveAdapterErrorCode::ConfigRejected { reason } => Self::ConfigRejected {
                reason: reason.into(),
            },
            LiveAdapterErrorCode::ProviderError => Self::ProviderError,
            LiveAdapterErrorCode::AuthenticationFailed => Self::AuthenticationFailed,
            LiveAdapterErrorCode::InternalError => Self::InternalError,
            LiveAdapterErrorCode::Other { raw } => Self::Other { raw },
            // Core enum is `#[non_exhaustive]`. R5-3 (P3): surface unknown
            // variants explicitly via `Unknown { debug }` rather than
            // silently coercing to `InternalError` (the previous fail-open
            // default — a plausible lie that would attribute every
            // future-variant failure to an internal-server bug). When a
            // new core variant lands, add an explicit arm above this
            // comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveAdapterErrorCode::from saw an unmapped \
                     LiveAdapterErrorCode variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
            }
        }
    }
}

impl TryFrom<WireLiveAdapterErrorCode> for LiveAdapterErrorCode {
    type Error = WireConversionError;

    fn try_from(value: WireLiveAdapterErrorCode) -> Result<Self, Self::Error> {
        // No wildcard arm: `WireLiveAdapterErrorCode` is owned by this
        // crate so even with `#[non_exhaustive]` the compiler enforces
        // exhaustive coverage here.
        match value {
            WireLiveAdapterErrorCode::ConnectionFailed => Ok(Self::ConnectionFailed),
            WireLiveAdapterErrorCode::ConnectionLost => Ok(Self::ConnectionLost),
            WireLiveAdapterErrorCode::ConfigRejected { reason } => Ok(Self::ConfigRejected {
                reason: reason.try_into()?,
            }),
            WireLiveAdapterErrorCode::ProviderError => Ok(Self::ProviderError),
            WireLiveAdapterErrorCode::AuthenticationFailed => Ok(Self::AuthenticationFailed),
            WireLiveAdapterErrorCode::InternalError => Ok(Self::InternalError),
            WireLiveAdapterErrorCode::Other { raw } => Ok(Self::Other { raw }),
            WireLiveAdapterErrorCode::Unknown { debug } => {
                Err(WireConversionError::ErrorCode { debug })
            }
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
        usage: crate::wire::WireUsage,
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
        usage: crate::wire::WireUsage,
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
    /// R3-6 (P2): explicit fail-loud variant for unknown core variants.
    ///
    /// The core [`LiveAdapterObservation`] enum is `#[non_exhaustive]`. When
    /// a future variant lands without an explicit arm in the wire `From`
    /// impl, the conversion surfaces as `Unknown { debug }` rather than
    /// silently coercing into the worst-possible default —
    /// `TurnInterrupted` — which would surface a real new event as an
    /// interrupt and drop data downstream. SDK consumers route on the
    /// `observation: "unknown"` discriminator and treat it as an
    /// unrecognized event — never as a barge-in. The `debug` payload
    /// carries the `{:?}` projection of the source variant so server logs
    /// name the real shape that needs to be promoted.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown {
        debug: String,
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
                usage: usage.into(),
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
                usage: usage.into(),
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
            // Core enum is `#[non_exhaustive]`. R3-6 (P2): surface unknown
            // variants explicitly via `Unknown { debug }` rather than
            // silently coercing to `TurnInterrupted` (the previous default
            // — the worst-possible fallback because a real new event would
            // surface as a barge-in and drop data downstream). When a new
            // core variant lands, add an explicit arm above this comment.
            other => {
                debug_assert!(
                    false,
                    "WireLiveAdapterObservation::from saw an unmapped \
                     LiveAdapterObservation variant; add an explicit arm in \
                     meerkat-contracts/src/wire/live.rs."
                );
                Self::Unknown {
                    debug: format!("{other:?}"),
                }
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
            turning_mode: None,
            transport: None,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        // R3-1: omitted `turning_mode` elides on the wire (back-compat).
        assert!(
            j.get("turning_mode").is_none(),
            "default `turning_mode` must elide the field on the wire"
        );
        let back: LiveOpenParams = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_open_params_explicit_commit_round_trip() {
        // R3-1 (P1): explicit-commit on the wire so the G9 typed text-only
        // commit_input path is reachable.
        let v = LiveOpenParams {
            session_id: "session-1".into(),
            turning_mode: Some(RealtimeTurningMode::ExplicitCommit),
            transport: None,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["turning_mode"], "explicit_commit");
        let back: LiveOpenParams = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_open_params_provider_managed_explicit_round_trip() {
        let v = LiveOpenParams {
            session_id: "session-1".into(),
            turning_mode: Some(RealtimeTurningMode::ProviderManaged),
            transport: None,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["turning_mode"], "provider_managed");
        let back: LiveOpenParams = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn live_open_params_webrtc_transport_round_trip() {
        let v = LiveOpenParams {
            session_id: "session-1".into(),
            turning_mode: None,
            transport: Some(LiveOpenTransport::Webrtc),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["transport"], "webrtc");
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
            let core: LiveContinuityMode = v
                .clone()
                .try_into()
                .expect("known wire variants must convert to core");
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
                usage: crate::wire::WireUsage {
                    input_tokens: 5,
                    output_tokens: 7,
                    total_tokens: 12,
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
                usage: crate::wire::WireUsage {
                    input_tokens: 12,
                    output_tokens: 34,
                    total_tokens: 46,
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
                    reason: WireLiveConfigRejectionReason::ImageInputNotImplemented,
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
                reason: WireLiveConfigRejectionReason::VideoFrameInputNotImplemented,
            },
            message: "rejected".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["observation"], "command_rejected");
        assert_eq!(j["code"]["code"], "config_rejected");
        // R5-2: `reason` is the typed enum, internally tagged on `kind`.
        assert_eq!(
            j["code"]["reason"]["kind"],
            "video_frame_input_not_implemented"
        );
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
                reason: LiveConfigRejectionReason::ImageInputNotImplemented,
            },
            message: "rejected".into(),
        };
        let wire: WireLiveAdapterObservation = core.clone().into();
        let core_json = serde_json::to_value(&core).expect("round-trip should succeed");
        let wire_json = serde_json::to_value(&wire).expect("round-trip should succeed");
        assert_eq!(core_json, wire_json);
    }

    // R3-6 (P2) — explicit Unknown variant fail-loud regression tests.

    #[test]
    fn unknown_observation_variant_does_not_become_turn_interrupted() {
        // R3-6 (P2): the wire `Unknown` variant must NOT serialize as
        // `TurnInterrupted` (the previous fail-open default), and a real
        // future-variant forward conversion must NOT collapse onto
        // `TurnInterrupted`. Synthesize the future-variant case by
        // constructing the wire `Unknown` sentinel directly.
        let v = WireLiveAdapterObservation::Unknown {
            debug: "FutureVariant { … }".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(
            j["observation"], "unknown",
            "wire Unknown must NOT serialize as turn_interrupted"
        );
        assert_ne!(
            j["observation"], "turn_interrupted",
            "wire Unknown must never coerce to turn_interrupted"
        );
        assert_eq!(j["debug"], "FutureVariant { … }");
        let back: WireLiveAdapterObservation =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    #[test]
    fn known_observation_variants_never_serialize_as_unknown() {
        // R3-6 (P2): the explicit-Unknown variant is a floor, not a
        // destination. Every known core variant must produce its typed
        // wire counterpart, never `Unknown`. Exercises the
        // `LiveAdapterObservation::TurnInterrupted` case specifically
        // because the previous fail-open default coerced unknown variants
        // INTO `TurnInterrupted` — the inverse reachability check confirms
        // the new explicit-Unknown path doesn't collide.
        let real_interrupt = LiveAdapterObservation::TurnInterrupted {
            response_id: Some("resp_real".into()),
        };
        let wire: WireLiveAdapterObservation = real_interrupt.into();
        match &wire {
            WireLiveAdapterObservation::TurnInterrupted { response_id } => {
                assert_eq!(response_id.as_deref(), Some("resp_real"));
            }
            other => panic!("real TurnInterrupted must stay TurnInterrupted, got {other:?}"),
        }
        let j = serde_json::to_value(&wire).expect("round-trip should succeed");
        assert_eq!(j["observation"], "turn_interrupted");
        assert_ne!(j["observation"], "unknown");
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
    fn wire_live_transport_bootstrap_webrtc_round_trip() {
        let v = WireLiveTransportBootstrap::Webrtc {
            token: "tok_webrtc".into(),
            answer_method: "live/webrtc/answer".into(),
            http_url: None,
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["transport"], "webrtc");
        assert_eq!(j["token"], "tok_webrtc");
        assert_eq!(j["answer_method"], "live/webrtc/answer");
        assert!(
            j.get("http_url").is_none(),
            "missing optional HTTP signaling must elide"
        );
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
    fn wire_live_transport_bootstrap_webrtc_byte_compatible_with_core() {
        let core = LiveTransportBootstrap::Webrtc {
            token: "tok_xyz".into(),
            answer_method: "live/webrtc/answer".into(),
            http_url: Some("https://example/live/webrtc/answer".into()),
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
        // R3-6 (P2): wire → core is now `TryFrom`; `Unknown` rejects.
        let core: LiveTransportBootstrap =
            LiveTransportBootstrap::try_from(v.clone()).expect("known wire variant should convert");
        let back: WireLiveTransportBootstrap = core.into();
        assert_eq!(v, back);
    }

    #[test]
    fn wire_live_transport_bootstrap_webrtc_round_trips_through_core() {
        let v = WireLiveTransportBootstrap::Webrtc {
            token: "tok_back".into(),
            answer_method: "live/webrtc/answer".into(),
            http_url: None,
        };
        let core: LiveTransportBootstrap =
            LiveTransportBootstrap::try_from(v.clone()).expect("known wire variant should convert");
        let back: WireLiveTransportBootstrap = core.into();
        assert_eq!(v, back);
    }

    #[test]
    fn live_webrtc_answer_params_and_result_round_trip() {
        let params = LiveWebrtcAnswerParams {
            channel_id: "ch_1".into(),
            token: "tok".into(),
            offer_sdp: "v=0\r\n".into(),
        };
        let params_json = serde_json::to_value(&params).expect("round-trip should succeed");
        assert_eq!(params_json["channel_id"], "ch_1");
        assert_eq!(params_json["offer_sdp"], "v=0\r\n");
        let params_back: LiveWebrtcAnswerParams =
            serde_json::from_value(params_json).expect("round-trip should succeed");
        assert_eq!(params, params_back);

        let result = LiveWebrtcAnswerResult {
            answer_sdp: "v=0\r\n".into(),
        };
        let result_json = serde_json::to_value(&result).expect("round-trip should succeed");
        assert_eq!(result_json["answer_sdp"], "v=0\r\n");
        let result_back: LiveWebrtcAnswerResult =
            serde_json::from_value(result_json).expect("round-trip should succeed");
        assert_eq!(result, result_back);
    }

    #[test]
    fn wire_live_transport_bootstrap_unknown_does_not_become_websocket() {
        // R3-6 (P2): the wire `Unknown` variant must NOT silently convert
        // back to a bogus `Websocket { url: "", token: "" }`. The inverse
        // path returns `WireConversionError::Transport` so callers
        // explicitly handle the unsupported transport.
        let unknown = WireLiveTransportBootstrap::Unknown {
            debug: "Webrtc { offer_sdp: \"v=0...\", terminator_url: \"https://example/whip\" }"
                .to_string(),
        };
        match LiveTransportBootstrap::try_from(unknown.clone()) {
            Err(WireConversionError::Transport { debug }) => {
                assert!(debug.contains("Webrtc"), "debug payload preserved");
            }
            other => panic!("unknown wire variant must not coerce to a core variant: {other:?}"),
        }
        // Round-trips through serde as the explicit `unknown` discriminator.
        let j = serde_json::to_value(&unknown).expect("round-trip should succeed");
        assert_eq!(j["transport"], "unknown");
        assert!(
            j.get("url").is_none(),
            "Unknown wire variant must NOT carry websocket fields — that was the whole bug"
        );
        let back: WireLiveTransportBootstrap =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(unknown, back);
    }

    #[test]
    fn unknown_transport_variant_round_trips_as_unknown() {
        // R3-6 (P2): synthesize the future-variant case by directly
        // constructing the wire `Unknown` sentinel. JSON round-trip
        // preserves the discriminator and `debug` payload.
        let v = WireLiveTransportBootstrap::Unknown {
            debug: "FutureVariant { … }".into(),
        };
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["transport"], "unknown");
        assert_eq!(j["debug"], "FutureVariant { … }");
        let back: WireLiveTransportBootstrap =
            serde_json::from_value(j).expect("round-trip should succeed");
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
            let j = serde_json::to_value(&v).expect("round-trip should succeed");
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
            let wire_json = serde_json::to_value(&wire).expect("round-trip should succeed");
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

    /// R4-5 (P3): the typed `LiveRefreshResult` round-trips through JSON
    /// preserving both the new typed `status` discriminator and the legacy
    /// `refresh_enqueued: true` back-compat field. The `status: queued`
    /// shape mirrors the only outcome `LiveAdapterHost::send_command`
    /// produces today.
    #[test]
    fn live_refresh_result_queued_round_trip() {
        let v = LiveRefreshResult::queued();
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        assert_eq!(j["status"], "queued");
        assert_eq!(j["refresh_enqueued"], serde_json::Value::Bool(true));
        let back: LiveRefreshResult = serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(v, back);
    }

    /// R4-5 (P3): legacy clients that pattern-match on the old untyped
    /// `{"refresh_enqueued": true}` shape continue to see the field, and
    /// new clients that route on `status` see the typed variant. Both
    /// surfaces are byte-coexistent on the same payload.
    #[test]
    fn live_refresh_result_back_compat_field_present() {
        let v = LiveRefreshResult::queued();
        let j = serde_json::to_value(&v).expect("round-trip should succeed");
        // Legacy: callers checking the boolean directly still pass.
        assert_eq!(
            j.get("refresh_enqueued"),
            Some(&serde_json::Value::Bool(true)),
            "back-compat `refresh_enqueued: true` must remain on the wire"
        );
        // New: typed status discriminator is also present.
        assert_eq!(
            j.get("status"),
            Some(&serde_json::Value::String("queued".into())),
            "typed `status: queued` must be present alongside the legacy field"
        );
    }

    // R5-3 (P3) — explicit Unknown variant fail-loud regression tests for
    // continuity / response_modality / status / error_code wire mirrors.

    #[test]
    fn unknown_continuity_does_not_become_fresh() {
        // R5-3 (P3): the wire `Unknown` variant must NOT convert back to
        // `Fresh` (the previous fail-open default). The inverse path
        // returns `WireConversionError::Continuity` so callers
        // route on the typed error rather than silently fabricating a
        // fresh-channel placeholder.
        let unknown = WireLiveContinuityMode::Unknown {
            debug: "FutureContinuity { … }".into(),
        };
        match LiveContinuityMode::try_from(unknown.clone()) {
            Err(WireConversionError::Continuity { debug }) => {
                assert!(
                    debug.contains("FutureContinuity"),
                    "debug payload preserved"
                );
            }
            other => panic!("unknown wire variant must not coerce to a core variant: {other:?}"),
        }
        // Round-trips through serde as the explicit `unknown` discriminator.
        let j = serde_json::to_value(&unknown).expect("round-trip should succeed");
        assert_eq!(j["mode"], "unknown");
        assert_ne!(j["mode"], "fresh", "Unknown must never serialize as fresh");
        assert!(
            j.get("provider_session_id").is_none(),
            "Unknown wire variant must NOT carry resume fields"
        );
        let back: WireLiveContinuityMode =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(unknown, back);
    }

    #[test]
    fn unknown_response_modality_does_not_become_audio() {
        // R5-3 (P3): the wire `Unknown` variant must NOT convert back to
        // `Audio` (the previous fail-open default that would route a
        // future text/structured response through the audio playback path
        // and drop content).
        let unknown = WireLiveResponseModality::Unknown {
            debug: "Structured { … }".into(),
        };
        match LiveResponseModality::try_from(unknown.clone()) {
            Err(WireConversionError::ResponseModality { debug }) => {
                assert!(debug.contains("Structured"), "debug payload preserved");
            }
            other => panic!("unknown wire variant must not coerce to a core variant: {other:?}"),
        }
        let j = serde_json::to_value(&unknown).expect("round-trip should succeed");
        assert_eq!(j["modality"], "unknown");
        assert_ne!(
            j["modality"], "audio",
            "Unknown must never serialize as audio"
        );
        let back: WireLiveResponseModality =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(unknown, back);
    }

    #[test]
    fn unknown_status_does_not_become_closed() {
        // R5-3 (P3): the wire `Unknown` variant must NOT serialize as
        // `closed` (the previous fail-open default — a plausible lie that
        // would tell consumers a healthy channel was torn down). No
        // inverse `From/TryFrom` is emitted today (matches the
        // `WireLiveAdapterObservation` precedent — wire-side status is a
        // downstream projection, never authority); the regression
        // assertion lives at the serialization layer.
        let unknown = WireLiveAdapterStatus::Unknown {
            debug: "Reconnecting { … }".into(),
        };
        let j = serde_json::to_value(&unknown).expect("round-trip should succeed");
        assert_eq!(j["status"], "unknown");
        assert_ne!(
            j["status"], "closed",
            "Unknown must never serialize as closed"
        );
        assert_eq!(j["debug"], "Reconnecting { … }");
        let back: WireLiveAdapterStatus =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(unknown, back);
    }

    #[test]
    fn unknown_error_code_does_not_become_internal_error() {
        // R5-3 (P3): the wire `Unknown` variant must NOT convert back to
        // `InternalError` (the previous fail-open default that would
        // attribute every future-variant failure to an internal-server
        // bug and mask real provider/transport classification).
        let unknown = WireLiveAdapterErrorCode::Unknown {
            debug: "QuotaExhausted { … }".into(),
        };
        match LiveAdapterErrorCode::try_from(unknown.clone()) {
            Err(WireConversionError::ErrorCode { debug }) => {
                assert!(debug.contains("QuotaExhausted"), "debug payload preserved");
            }
            other => panic!("unknown wire variant must not coerce to a core variant: {other:?}"),
        }
        let j = serde_json::to_value(&unknown).expect("round-trip should succeed");
        assert_eq!(j["code"], "unknown");
        assert_ne!(
            j["code"], "internal_error",
            "Unknown must never serialize as internal_error"
        );
        let back: WireLiveAdapterErrorCode =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(unknown, back);
    }

    // R6-5 (P3 dogma) — explicit Unknown variant fail-loud regression tests
    // for the WireLiveConfigRejectionReason mirror.

    #[test]
    fn unknown_config_rejection_reason_does_not_become_other() {
        // R6-5 (P3): the wire `Unknown` variant must NOT convert back to
        // `Other { detail: "..." }` (the previous fail-open default — a
        // "typed route becomes detail string" antipattern that forced SDK
        // consumers to parse English from `detail` to recover the missing
        // route).
        let unknown = WireLiveConfigRejectionReason::Unknown {
            debug: "FuturePolicyRejection { … }".into(),
        };
        match LiveConfigRejectionReason::try_from(unknown.clone()) {
            Err(WireConversionError::ConfigRejectionReason { debug }) => {
                assert!(
                    debug.contains("FuturePolicyRejection"),
                    "debug payload preserved"
                );
            }
            other => panic!("unknown wire variant must not coerce to a core variant: {other:?}"),
        }
        // Round-trips through serde as the explicit `unknown` discriminator.
        let j = serde_json::to_value(&unknown).expect("round-trip should succeed");
        assert_eq!(j["kind"], "unknown");
        assert_ne!(
            j["kind"], "other",
            "Unknown must never serialize as `other`"
        );
        assert!(
            j.get("detail").is_none(),
            "Unknown wire variant must NOT carry an `Other.detail` field"
        );
        assert_eq!(j["debug"], "FuturePolicyRejection { … }");
        let back: WireLiveConfigRejectionReason =
            serde_json::from_value(j).expect("round-trip should succeed");
        assert_eq!(unknown, back);
    }

    #[test]
    fn known_config_rejection_reason_variants_never_serialize_as_unknown() {
        // R6-5 (P3): the explicit-Unknown variant is a floor, not a
        // destination. Every known core variant must produce its typed
        // wire counterpart, never `Unknown`.
        let real_other = LiveConfigRejectionReason::Other {
            detail: "real explanation".into(),
        };
        let wire: WireLiveConfigRejectionReason = real_other.into();
        match &wire {
            WireLiveConfigRejectionReason::Other { detail } => {
                assert_eq!(detail, "real explanation");
            }
            other => panic!("real Other must stay Other, got {other:?}"),
        }
        let j = serde_json::to_value(&wire).expect("round-trip should succeed");
        assert_eq!(j["kind"], "other");
        assert_ne!(j["kind"], "unknown");
    }

    #[test]
    fn config_rejection_reason_round_trips_through_core() {
        // R6-5 (P3): every known wire variant must round-trip through
        // core via the `TryFrom` inverse.
        let cases = [
            WireLiveConfigRejectionReason::ImageInputNotImplemented,
            WireLiveConfigRejectionReason::VideoFrameInputNotImplemented,
            WireLiveConfigRejectionReason::UnsupportedInputChunkVariant,
            WireLiveConfigRejectionReason::NonRealtimeResolution {
                detail: "not realtime".into(),
            },
            WireLiveConfigRejectionReason::RefreshModelSwap {
                from_model: "a".into(),
                to_model: "b".into(),
            },
            WireLiveConfigRejectionReason::AudioInputFormatMismatch {
                expected_sample_rate_hz: 24_000,
                expected_channels: 1,
                actual_sample_rate_hz: 16_000,
                actual_channels: 2,
            },
            WireLiveConfigRejectionReason::ChannelIdentitySwap {
                from_model: "claude-opus-4-6".into(),
                from_provider: WireProvider::Anthropic,
                to_model: "gpt-5.4".into(),
                to_provider: WireProvider::OpenAi,
            },
            WireLiveConfigRejectionReason::Other {
                detail: "anything".into(),
            },
        ];
        for v in cases {
            let core: LiveConfigRejectionReason = v
                .clone()
                .try_into()
                .expect("known wire variant should convert");
            let back: WireLiveConfigRejectionReason = core.into();
            assert_eq!(v, back);
        }
    }

    // --- WireProvider regression tests ---

    #[test]
    fn wire_provider_openai_serializes_as_openai() {
        // P2 regression: core `Provider::OpenAI` with `rename_all = "snake_case"`
        // serializes as `"open_a_i"`. `WireProvider::OpenAi` must serialize as
        // `"openai"` on the wire.
        let v = WireProvider::OpenAi;
        let j = serde_json::to_value(&v).expect("serialization should succeed");
        assert_eq!(
            j, "openai",
            "WireProvider::OpenAi must serialize as \"openai\", not \"open_a_i\""
        );
    }

    #[test]
    fn wire_provider_all_known_variants_round_trip() {
        let cases = [
            (WireProvider::Anthropic, "anthropic"),
            (WireProvider::OpenAi, "openai"),
            (WireProvider::Gemini, "gemini"),
            (WireProvider::SelfHosted, "self_hosted"),
            (WireProvider::Other, "other"),
        ];
        for (variant, expected_str) in cases {
            let j = serde_json::to_value(&variant).expect("serialization should succeed");
            assert_eq!(j, expected_str, "variant {variant:?} wrong wire name");
            let back: WireProvider =
                serde_json::from_value(j).expect("deserialization should succeed");
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn wire_provider_from_core_round_trips() {
        let cases = [
            (Provider::Anthropic, WireProvider::Anthropic),
            (Provider::OpenAI, WireProvider::OpenAi),
            (Provider::Gemini, WireProvider::Gemini),
            (Provider::SelfHosted, WireProvider::SelfHosted),
            (Provider::Other, WireProvider::Other),
        ];
        for (core, expected_wire) in cases {
            let wire: WireProvider = core.into();
            assert_eq!(wire, expected_wire);
            let back: Provider = wire.try_into().expect("known wire variant should convert");
            assert_eq!(back, core);
        }
    }

    #[test]
    fn wire_provider_unknown_does_not_become_known_variant() {
        let unknown = WireProvider::Unknown;
        let j = serde_json::to_value(&unknown).expect("serialization should succeed");
        assert_eq!(
            j, "unknown",
            "WireProvider::Unknown must serialize as \"unknown\""
        );
        let back: WireProvider = serde_json::from_value(j).expect("deserialization should succeed");
        assert_eq!(unknown, back);
        match Provider::try_from(unknown) {
            Err(WireConversionError::Provider { .. }) => {
                // Expected: Unknown has no core counterpart.
            }
            other => panic!("unknown wire variant must not coerce to a core variant: {other:?}"),
        }
    }

    #[test]
    fn channel_identity_swap_serializes_provider_correctly() {
        // P2 regression: the old shape exposed core `Provider` which
        // serialized `OpenAI` as `"open_a_i"`. The new `WireProvider`
        // must serialize as `"openai"`.
        let v = WireLiveConfigRejectionReason::ChannelIdentitySwap {
            from_model: "claude-opus-4-6".into(),
            from_provider: WireProvider::Anthropic,
            to_model: "gpt-5.4".into(),
            to_provider: WireProvider::OpenAi,
        };
        let j = serde_json::to_value(&v).expect("serialization should succeed");
        assert_eq!(j["from_provider"], "anthropic");
        assert_eq!(j["to_provider"], "openai");
        // Must NOT serialize as "open_a_i"
        assert_ne!(
            j["to_provider"], "open_a_i",
            "WireProvider must use explicit rename, not snake_case"
        );
    }
}
