//! Live adapter seam vocabulary.
//!
//! Defines the typed command/observation boundary between Meerkat runtime
//! (which owns semantic authority) and live provider adapters (which own
//! provider transport mechanics). This is the single language both sides
//! speak — no provider-specific types cross this boundary.

use std::borrow::Cow;

#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::provider::Provider;
use crate::realtime_transcript::RealtimeTranscriptEvent;
use crate::session::PendingSystemContextAppend;
use crate::types::{ContentBlock, Message, SessionId, StopReason, ToolDef, Usage};

// ---------------------------------------------------------------------------
// Adapter status — typed lifecycle, not Option<T> hiding five states
// ---------------------------------------------------------------------------

/// Typed lifecycle phase of a live adapter session.
///
/// Replaces `Option<provider_session>` patterns where `None` means five
/// different things (dogma sin #7). Each variant is a distinct semantic
/// state the adapter host can act on.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterStatus {
    Idle,
    Opening,
    Ready,
    Degraded { reason: LiveDegradationReason },
    Closing,
    Closed,
}

impl LiveAdapterStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Closed)
    }

    #[must_use]
    pub fn accepts_commands(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Typed reason for adapter degradation. Replaces a free-form `String`
/// so callers can route on the cause without parsing English (dogma sin #1).
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveDegradationReason {
    RateLimited,
    ProviderThrottled,
    NetworkUnstable,
    Other {
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        detail: Cow<'static, str>,
    },
}

// ---------------------------------------------------------------------------
// Seam-owned tool result — adapter doesn't need ContentBlock knowledge
// ---------------------------------------------------------------------------

/// Tool result at the adapter seam. Carries structured content blocks so
/// callers can preserve fidelity (text, image, video) instead of stringifying
/// into a single text block.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveToolResult {
    pub call_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Commands — Meerkat runtime → adapter
// ---------------------------------------------------------------------------

/// G9: per-response output modality override for `LiveAdapterCommand::CommitInput`.
///
/// Live channels run with a session-level modality (`Audio` for the OpenAI
/// realtime adapter — see `session_update_with_audio_text_modality`). The
/// OpenAI realtime API supports per-response overrides via `response.create`'s
/// `output_modalities` field, so a caller that wants a single text-only
/// response on an otherwise-audio channel can pass `Some(LiveResponseModality::Text)`
/// without tearing down and reopening the channel.
///
/// Adapters that cannot honor a particular modality (e.g. a hypothetical
/// audio-only provider) must surface a typed `ConfigRejected` rejection
/// rather than silently dropping the override.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "modality", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveResponseModality {
    /// Spoken-audio plus the audio-derived transcript. The default on
    /// audio-first live channels (e.g. OpenAI realtime).
    Audio,
    /// Display text only — no audio output, no transcript. The OpenAI
    /// realtime adapter maps this to `output_modalities=Text` on
    /// `response.create`. Callers that want to suppress audio for a single
    /// response (e.g. an interstitial summary) without flipping the
    /// channel modality use this variant.
    Text,
}

/// Commands sent from the Meerkat runtime to a live provider adapter.
///
/// The adapter host gates these through semantic authority checks before
/// dispatch. The adapter treats them as instructions, not truth — it does
/// not decide whether an interrupt is legal or a tool result is authorized.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterCommand {
    /// R2: reserved for cross-session re-seed scenarios (resume,
    /// cross-session attach) where the runtime hands an already-bound
    /// adapter a snapshot that was *not* seeded at factory-construction
    /// time. `live/open` does NOT dispatch this — `factory.open_live_adapter`
    /// already passed `seed_messages` + `runtime_system_context` to the
    /// provider session before adapter wrap, so dispatching `Open` again
    /// would double-seed the conversation. Adapters that receive this
    /// command after the channel is already live must replay the snapshot's
    /// seed history into their provider session (the OpenAI arm does this
    /// via `seed_history_projection`).
    Open {
        snapshot: LiveProjectionSnapshot,
    },
    /// P1#5: refresh an *already-open* adapter's mutable config against a
    /// freshly built [`LiveProjectionSnapshot`], without replaying history.
    ///
    /// Triggered when upstream session state changes (config patch that
    /// alters instructions / tools / audio, snapshot drift after a session
    /// edit, etc.) and the runtime wants the live adapter to re-apply the
    /// mutable configuration fields on the provider session without
    /// tearing the channel down.
    ///
    /// R9 invariant: refresh MUST NOT re-seed the conversation history.
    /// The `Open` arm is the single authoritative seed point — re-running
    /// `seed_history_projection` here would duplicate every prior turn
    /// into the provider's hosted conversation on each refresh. The
    /// OpenAI adapter therefore handles `Refresh` by validating model
    /// identity (model swaps require close + reopen — OpenAI Realtime has
    /// no mutable `model` field) and issuing a single `session.update`
    /// carrying the new instructions / tools / audio config. The
    /// snapshot's `runtime_system_context` is folded into `instructions`
    /// by that update, so newly-authoritative system context still reaches
    /// the provider — as session config, not as synthetic conversation
    /// items.
    ///
    /// Adapters that do not support live re-config should treat this as a
    /// no-op or surface a typed error observation.
    Refresh {
        snapshot: LiveProjectionSnapshot,
    },
    SendInput {
        chunk: LiveInputChunk,
    },
    /// G9: commit any buffered input and request a response. Optional
    /// `response_modality` lets the caller pick between the channel's
    /// default modality (`None`) and an explicit override (`Text` for a
    /// text-only response, `Audio` for spoken+transcript). On the OpenAI
    /// realtime surface the override flows into `response.create`'s
    /// `output_modalities` field, which the provider honors per-response
    /// even when the session-level modality is `Audio`.
    ///
    /// Adapters that do not honor a particular modality must surface a
    /// typed `ConfigRejected` rejection rather than silently dropping the
    /// override.
    CommitInput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_modality: Option<LiveResponseModality>,
    },
    Interrupt,
    TruncateAssistantOutput {
        item_id: String,
        content_index: u32,
        audio_played_ms: u64,
    },
    SubmitToolResult {
        result: LiveToolResult,
    },
    SubmitToolError {
        call_id: String,
        error: String,
    },
    Close,
}

// ---------------------------------------------------------------------------
// Observations — adapter → Meerkat runtime
// ---------------------------------------------------------------------------

/// Observations emitted by a live provider adapter back to the Meerkat runtime.
///
/// These are facts the adapter observed, not decisions. The runtime decides
/// what they mean for canonical session state (dogma #2: machines own
/// semantics). Provider IDs are opaque diagnostic references, not control
/// handles (dogma sin #6).
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterObservation {
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
    /// Streaming **display** text delta from the provider. Distinct from
    /// [`Self::AssistantTranscriptDelta`]: this lane carries authored text
    /// the provider emits as primary output (`response.output_text.delta` on
    /// the OpenAI realtime surface), not audio-derived speech transcription.
    ///
    /// The runtime flushes these into [`crate::types::AssistantBlock::Text`].
    /// Display text is preserved across barge-in (the user is not "speaking
    /// over" written output).
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
    /// Streaming **spoken-transcript** delta — text derived from the
    /// provider's audio output (`response.output_audio_transcript.delta` on
    /// the OpenAI realtime surface). Distinct from
    /// [`Self::AssistantTextDelta`] because it represents what the model
    /// *said*, not what it *wrote*; the runtime flushes these into
    /// [`crate::types::AssistantBlock::Transcript`] with
    /// `source: TranscriptSource::Spoken`.
    ///
    /// Identity shape mirrors `AssistantTextDelta` so per-response buffering
    /// (R6 `(SessionId, Option<response_id>)`) and barge-in scoping
    /// ([`Self::TurnInterrupted`]) apply uniformly.
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
    /// Streaming audio frame. R5-4: identity fields (`response_id`,
    /// `item_id`, `content_index`) propagate the source server-event identity
    /// the OpenAI realtime translator already records internally so clients
    /// can attach a playback cursor to a provider item without racing on the
    /// transcript-delta arrival order. All three are `Option` because not
    /// every provider surfaces a content segment id and degraded paths may
    /// drop response identity.
    AssistantAudioChunk {
        #[serde(with = "base64_bytes")]
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        data: Vec<u8>,
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
        stop_reason: StopReason,
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
    /// Pass-through of a structured `RealtimeTranscriptEvent` from the provider.
    ///
    /// The adapter forwards these as-is so the runtime's projection layer can
    /// reconstruct authoritative transcript state without the adapter having
    /// to interpret the event. Replaces the lossy fold into `StatusChanged`.
    RealtimeTranscript {
        event: RealtimeTranscriptEvent,
    },
    ToolCallRequested {
        provider_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Barge-in: the user interrupted the assistant mid-turn.
    ///
    /// Scope (T7): this observation invalidates **only** spoken-transcript
    /// and audio-playback state for the in-flight turn. Display-text state
    /// (the [`Self::AssistantTextDelta`] / [`AssistantBlock::Text`] lane) is
    /// **preserved** — the user did not speak over written output, so any
    /// already-buffered display-text final still commits at
    /// [`Self::TurnCompleted`] (or via the projection sink's text-only
    /// flush). Sinks must drain transcript buffers and any audio playback
    /// state at this signal but leave the display-text buffer alone.
    ///
    /// G4 (P1): `response_id` carries the provider's identifier for the
    /// in-flight response that was interrupted. Without this, projection
    /// sinks have to *infer* the affected response from staged transcript
    /// state — and a barge-in that lands before any transcript delta
    /// (very fast user interjection) leaves the sink with no staged
    /// response to truncate against. Optional because not every provider
    /// surfaces an opaque response id, and the truncation path may be
    /// reachable from synthetic / degraded sources without a known id.
    TurnInterrupted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
    },
    TurnCompleted {
        /// R6: the provider's response identifier for the turn that just
        /// completed. The OpenAI realtime API delivers `response.done` with a
        /// `response_id`; without plumbing this through, the projection sink
        /// can only key buffered finals by `SessionId`, which lets a stale or
        /// overlapping `response.done` flush the wrong buffered transcript.
        /// Optional because not every provider surfaces an opaque response id
        /// (transcript-only adapters, degraded ack paths).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        stop_reason: StopReason,
        usage: Usage,
    },
    StatusChanged {
        status: LiveAdapterStatus,
    },
    Error {
        code: LiveAdapterErrorCode,
        message: String,
    },
    /// R5-9: scoped command rejection. The adapter rejected an in-flight
    /// command (e.g. unsupported `LiveInputChunk` shape) but the channel
    /// remains usable. Distinct from [`Self::Error`] — the host treats
    /// `Error` as terminal (closes the channel) and `CommandRejected` as a
    /// per-command failure (channel survives, client may retry with a
    /// supported shape). Terminal failure paths (model swap, audio-rate
    /// mismatch, provider outage) continue to use `Error`.
    CommandRejected {
        code: LiveAdapterErrorCode,
        message: String,
    },
}

/// Serde helper: serialize `Vec<u8>` as a base64 (standard) string instead of
/// a JSON integer array. Required for audio chunks: a u8 array carries ~6×
/// overhead and forces JSON parse on every audio frame.
mod base64_bytes {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = BASE64_STANDARD.encode(bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        BASE64_STANDARD
            .decode(encoded.as_ref())
            .map_err(serde::de::Error::custom)
    }
}

/// Typed error codes for adapter-level failures. Transport/provider errors
/// are not Meerkat terminal outcomes (dogma sin #3) — the runtime decides
/// the semantic consequence.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterErrorCode {
    ConnectionFailed,
    ConnectionLost,
    /// R12: a local guard (model swap, provider swap, audio-rate mismatch,
    /// etc.) rejected an adapter command without provider involvement.
    ///
    /// Distinct from [`Self::ProviderError`], which signals the realtime
    /// provider returned an error. `ConfigRejected` is the signal that the
    /// runtime/adapter pre-flight refused to forward the command — typically
    /// because the requested change cannot be applied via in-place
    /// reconfigure (OpenAI realtime's `session.update` cannot change models)
    /// and the caller must close + reopen the channel. Clients that
    /// previously had to parse English from `ProviderError` messages can now
    /// route on the typed code.
    ///
    /// R5-2 (P2 dogma): the carried reason is a typed
    /// [`LiveConfigRejectionReason`], not a free-form `String`. Callers
    /// route on the variant; the variant's `Display` impl produces a
    /// human-readable summary for logs / WS close-frame `message` fields.
    ConfigRejected {
        reason: LiveConfigRejectionReason,
    },
    ProviderError,
    AuthenticationFailed,
    InternalError,
    /// Unknown provider error code preserved verbatim for diagnostics.
    Other {
        raw: String,
    },
}

/// Typed semantic reasons a [`LiveAdapterErrorCode::ConfigRejected`] is
/// raised. R5-2 (P2 dogma): replaces the previous free-form `String` so
/// downstream consumers can route on the variant rather than parsing
/// English from a diagnostic blob.
///
/// Variants split into:
///
/// * **Runtime-side identity gates** (the runtime detected the live
///   channel's bound identity diverges from the session's resolved
///   identity, or a per-session post-`config/patch` precheck failed):
///   [`Self::ChannelIdentitySwap`], [`Self::NonRealtimeResolution`].
/// * **Adapter-side input-shape gates** (the adapter cannot deliver the
///   requested input modality on the bound provider):
///   [`Self::ImageInputNotImplemented`],
///   [`Self::VideoFrameInputNotImplemented`],
///   [`Self::UnsupportedInputChunkVariant`].
/// * **Adapter-side refresh gates** (the adapter rejected an in-place
///   `session.update`-style refresh because the snapshot mutated a field
///   the provider session cannot rebind in place):
///   [`Self::RefreshModelSwap`], [`Self::RefreshProviderSwap`],
///   [`Self::RefreshAudioConfigMismatch`],
///   [`Self::AudioInputFormatMismatch`].
/// * **Diagnostic catch-all**: [`Self::Other`] for one-off explanations
///   not yet worth typing. New routing-relevant cases must add a typed
///   variant rather than leaning on [`Self::Other`].
///
/// Wire shape: internally tagged on `kind` (snake_case) so SDK consumers
/// can route on the discriminator. `#[non_exhaustive]` so future variants
/// don't break existing callers.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveConfigRejectionReason {
    /// Runtime detected the live channel's bound LLM identity diverges
    /// from the session's resolved identity (typically after a global
    /// `config/patch` model/provider swap). The channel cannot rebind
    /// in place; the SDK must close + reopen against the new identity.
    ChannelIdentitySwap {
        from_model: String,
        from_provider: Provider,
        to_model: String,
        to_provider: Provider,
    },
    /// Per-session post-`config/patch` precheck rejected the channel
    /// because the new resolved model is not realtime-capable, or the
    /// new provider has no live adapter implementation. `detail` is a
    /// best-effort projection of the underlying
    /// `LiveOpenPrecheckError` for logs.
    NonRealtimeResolution { detail: String },
    /// Adapter rejected a `LiveInputChunk::Image` because the bound
    /// provider does not support image input on its realtime surface.
    ImageInputNotImplemented,
    /// Adapter rejected a `LiveInputChunk::VideoFrame` because the bound
    /// provider does not support video-frame input on its realtime
    /// surface.
    VideoFrameInputNotImplemented,
    /// Adapter rejected a future `LiveInputChunk` variant the bound
    /// provider does not yet implement. `LiveInputChunk` is
    /// `#[non_exhaustive]`; this variant covers the same shape on the
    /// rejection side.
    UnsupportedInputChunkVariant,
    /// Adapter-side `Refresh` rejected because the snapshot's model
    /// differs from the bound model — provider session.update cannot
    /// remap model in place; close + reopen required.
    RefreshModelSwap {
        from_model: String,
        to_model: String,
    },
    /// Adapter-side `Refresh` rejected because the snapshot's provider
    /// differs from the bound provider — close + reopen required.
    RefreshProviderSwap {
        from_provider: String,
        to_provider: String,
    },
    /// Adapter-side `Refresh` rejected because the snapshot's audio
    /// config cannot be applied in place (e.g. OpenAI Realtime's session
    /// is fixed to pcm/24kHz mono). `detail` carries the offending
    /// rate/channel projection for logs.
    RefreshAudioConfigMismatch { detail: String },
    /// R6-4 (P2): adapter rejected a `SendInput { LiveInputChunk::Audio }`
    /// chunk whose declared `sample_rate_hz` / `channels` diverge from
    /// the bound provider session's fixed audio format (e.g. OpenAI
    /// Realtime is fixed to PCM 24 kHz mono). Without this typed gate
    /// the chunk's bytes would be appended into the provider buffer
    /// under the session's actual format and the caller would get a
    /// "sent" success while the server interpreted the bytes at the
    /// wrong rate/channel count — silent semantic loss. SDK consumers
    /// route on the typed variant to surface a precise format-mismatch
    /// diagnostic instead of parsing English from `Other.detail`.
    AudioInputFormatMismatch {
        expected_sample_rate_hz: u32,
        expected_channels: u16,
        actual_sample_rate_hz: u32,
        actual_channels: u16,
    },
    /// Diagnostic catch-all for free-form explanations not yet worth
    /// typing. Add a typed variant before reaching for this; callers
    /// must not pattern-match on `detail` for routing.
    Other { detail: String },
}

impl std::fmt::Display for LiveConfigRejectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChannelIdentitySwap {
                from_model,
                from_provider,
                to_model,
                to_provider,
            } => {
                write!(
                    f,
                    "model_swap: {from_model} ({from_provider:?}) -> {to_model} ({to_provider:?})"
                )
            }
            Self::NonRealtimeResolution { detail } => {
                write!(f, "non_realtime_resolution: {detail}")
            }
            Self::ImageInputNotImplemented => f.write_str("image_input_not_implemented"),
            Self::VideoFrameInputNotImplemented => f.write_str("video_frame_input_not_implemented"),
            Self::UnsupportedInputChunkVariant => f.write_str("unsupported_input_chunk_variant"),
            Self::RefreshModelSwap {
                from_model,
                to_model,
            } => write!(
                f,
                "live adapter refresh: model swap from `{from_model}` to `{to_model}` requires close + reopen \
                 (provider session.update cannot rebind model in place)"
            ),
            Self::RefreshProviderSwap {
                from_provider,
                to_provider,
            } => write!(
                f,
                "live adapter refresh: provider swap from `{from_provider}` to `{to_provider}` requires close + reopen"
            ),
            Self::RefreshAudioConfigMismatch { detail } => {
                write!(f, "live adapter refresh: audio config mismatch ({detail})")
            }
            Self::AudioInputFormatMismatch {
                expected_sample_rate_hz,
                expected_channels,
                actual_sample_rate_hz,
                actual_channels,
            } => write!(
                f,
                "audio_input_format_mismatch: chunk declared rate={actual_sample_rate_hz}Hz \
                 ch={actual_channels} but bound provider session is fixed at \
                 rate={expected_sample_rate_hz}Hz ch={expected_channels}"
            ),
            Self::Other { detail } => f.write_str(detail),
        }
    }
}

// ---------------------------------------------------------------------------
// Projection snapshot — canonical Meerkat state projected for adapter use
// ---------------------------------------------------------------------------

/// Canonical Meerkat session state projected for a live provider adapter.
///
/// This is a read-only projection, not authority (dogma #13). The adapter
/// uses it to seed or rebuild its provider session. Staleness is explicit:
/// `snapshot_version` is a monotonic counter the adapter host increments on
/// every rebuild so the adapter can detect stale snapshots.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveProjectionSnapshot {
    pub session_id: SessionId,
    pub snapshot_version: u64,
    // `Message` does not derive `JsonSchema` (hand-rolled `Serialize` impl in
    // `crate::types`); represent as opaque JSON in the emitted schema until
    // upstream derives `JsonSchema`.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub seed_messages: Vec<Message>,
    // `ToolDef` does not yet derive `JsonSchema`; same treatment.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub visible_tools: Vec<ToolDef>,
    pub system_prompt: Option<String>,
    pub model_id: String,
    pub provider_id: String,
    pub audio_config: Option<LiveAudioConfig>,
    /// R3: typed runtime system-context entries projected alongside seed
    /// history.
    ///
    /// `RealtimeSessionOpenConfig` keeps `runtime_system_context` separate
    /// from `seed_messages` so adapters can fold the entries into their
    /// provider session as authoritative system instructions (peer terminal
    /// context, ops_lifecycle context, etc.) rather than mixing them into
    /// the canonical history. The snapshot must preserve that separation;
    /// flattening into `seed_messages` would lose provenance and trip the
    /// realtime layer's idempotent ordering on cross-source dedup.
    ///
    /// `PendingSystemContextAppend` does not (yet) derive `JsonSchema`;
    /// represent as opaque JSON in the emitted schema until upstream
    /// derives it (matches the treatment used for `Message` and `ToolDef`
    /// above).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub runtime_system_context: Vec<PendingSystemContextAppend>,
}

/// Audio format configuration for the live session.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveAudioConfig {
    pub input_sample_rate_hz: u32,
    pub input_channels: u16,
    pub output_sample_rate_hz: u32,
    pub output_channels: u16,
}

// ---------------------------------------------------------------------------
// Input chunks — modality-neutral admitted input
// ---------------------------------------------------------------------------

/// A modality-neutral input chunk admitted by Meerkat for delivery to the
/// provider. Meerkat already decided this input is admitted; the adapter
/// just delivers it.
///
/// T11: image and video-frame variants are typed at this seam so future
/// provider support (gpt-realtime-2 image input, Gemini Live video input)
/// flows through `live/send_input` without a wire reshape. Adapters that do
/// not yet support a variant must reject the command with
/// [`LiveAdapterErrorCode::ConfigRejected`] carrying a typed `reason`
/// (`"image_input_not_implemented"` / `"video_frame_input_not_implemented"`)
/// rather than collapsing onto a free-form provider error string.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveInputChunk {
    Audio {
        #[serde(with = "base64_bytes")]
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        data: Vec<u8>,
        sample_rate_hz: u32,
        channels: u16,
    },
    Text {
        text: String,
    },
    /// Image input (e.g. future `gpt-realtime-2` image support). `mime` is
    /// the IANA MIME type of the encoded bytes (`image/png`, `image/jpeg`,
    /// `image/webp`, …); `data` is the raw encoded bytes — base64-encoded
    /// on the wire (mirrors the [`Self::Audio`] / `AssistantAudioChunk`
    /// pattern) but a plain `Vec<u8>` in Rust so adapters never see the
    /// b64 wrapper.
    Image {
        mime: String,
        #[serde(with = "base64_bytes")]
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        data: Vec<u8>,
    },
    /// Single video-frame input (e.g. Gemini Live). `codec` is the encoding
    /// of the frame bytes (`vp8`, `vp9`, `h264`, `image/jpeg` for
    /// keyframe-as-image transports, …); `data` is the encoded frame
    /// payload (base64 on the wire, raw `Vec<u8>` in Rust);
    /// `timestamp_ms` is the presentation timestamp the adapter should
    /// stamp into the provider envelope so frames remain ordered.
    VideoFrame {
        codec: String,
        #[serde(with = "base64_bytes")]
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        data: Vec<u8>,
        timestamp_ms: u64,
    },
}

// ---------------------------------------------------------------------------
// Transport bootstrap — tagged transport info for surface API
// ---------------------------------------------------------------------------

/// Tagged transport bootstrap returned when opening a live channel.
///
/// The surface API returns this instead of a bare `ws_url` so Meerkat
/// semantics do not depend on transport kind (dogma sin #10). The enum is
/// `#[non_exhaustive]` so additional transports can be added without
/// breaking downstream consumers.
///
/// WebRTC uses browser-offer signaling: the browser creates its
/// `RTCPeerConnection`, captures local audio with AEC-capable constraints,
/// creates an SDP offer, then calls the returned JSON-RPC answer method
/// with `{ channel_id, token, offer_sdp }`. Meerkat answers and binds the
/// resulting media/data channels to the already-open live channel.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveTransportBootstrap {
    Websocket {
        url: String,
        token: String,
    },
    Webrtc {
        token: String,
        answer_method: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_url: Option<String>,
    },
}

/// Capabilities advertised when a live channel opens.
///
/// T8: typed-boolean matrix only — no `input_kinds` / `output_kinds` lists.
/// New modalities (image input on `gpt-realtime-2`, video input on Gemini
/// Live) appear as additional typed booleans without provider-specific
/// fields. Consumers grep for the exact capability they care about; lists
/// of stringly-typed `RealtimeInputKind` are only used inside the
/// provider-session capability projection (`RealtimeCapabilities`), not at
/// this surface boundary.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveChannelCapabilities {
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
    /// only resume does not count). `false` until a provider exposes a
    /// real continuation handle.
    pub provider_native_resume: bool,
}

impl Default for LiveChannelCapabilities {
    /// No-claim baseline: every capability defaults to `false`. Callers
    /// must opt in to each capability they actually support. This makes
    /// implicit "we support everything" advertisements impossible — every
    /// `true` in the matrix has a named source. A provider integration
    /// that forgets to claim a capability will appear narrow rather than
    /// silently overstating support.
    fn default() -> Self {
        Self {
            audio_in: false,
            audio_out: false,
            text_in: false,
            text_out: false,
            image_in: false,
            video_in: false,
            transcript_supported: false,
            barge_in_supported: false,
            provider_native_resume: false,
        }
    }
}

/// Full response when a live channel is opened.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveChannelOpenResponse {
    pub transport: LiveTransportBootstrap,
    pub input_audio_format: Option<LiveAudioConfig>,
    pub capabilities: LiveChannelCapabilities,
    pub continuity: LiveContinuityMode,
}

/// Explicit continuity classification (dogma sin #8: no resume lies).
///
/// Used in the [`LiveChannelOpenResponse`] so callers can distinguish how
/// much state the new live channel actually inherits from prior canonical
/// session history. The variants are ordered by *fidelity*, from no
/// continuity (a brand-new channel) up to provider-native resume (the
/// strongest form of continuation Meerkat can currently express):
///
/// - [`Self::Fresh`] — a brand-new live channel with no prior history.
///   The adapter has no seed messages and no provider session id to attach
///   to; the first turn starts from a clean slate.
/// - [`Self::TranscriptOnly`] — history seeded from canonical transcript
///   only. Honest about the loss of audio tone, pronunciation,
///   partial-playback cursor position, and any provider-native state
///   (cached tool reasoning, attention KV, intra-response buffers). The
///   conversation is *semantically* continuous, but the model rebuilds its
///   acoustic / latent state from text.
/// - [`Self::Degraded`] — history seeded but with known gaps (e.g.
///   canonical replay failed mid-way, or a tool call's structured output
///   was lost between sessions). The caller is told continuity is partial
///   so user-facing UI can warn.
/// - [`Self::ProviderNativeResume { provider_session_id }`] — the provider
///   surfaced a session id we attached back to. This is the *only* mode
///   that preserves provider-native state across reconnects (audio tone,
///   pronunciation, cached attention, partial playback cursor, etc.). No
///   provider Meerkat ships today returns a usable resume id, so this
///   variant is reserved for future wiring; the open-time helper in
///   `meerkat-rpc` never synthesizes it.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveContinuityMode {
    Fresh,
    TranscriptOnly,
    Degraded,
    ProviderNativeResume { provider_session_id: String },
}

// ---------------------------------------------------------------------------
// Adapter trait — the seam contract
// ---------------------------------------------------------------------------

/// The adapter seam: provider-specific transport mechanics behind a
/// provider-neutral typed boundary.
///
/// Implementations own provider connection lifecycle, wire format
/// translation, and transport-specific quirks. They do NOT own:
/// - canonical transcript truth
/// - tool dispatch authority
/// - session lifecycle semantics
/// - mob routing
/// - realm/auth checks
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait LiveAdapter: Send + Sync {
    async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError>;

    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError>;

    fn status(&self) -> LiveAdapterStatus;

    async fn close(&self) -> Result<(), LiveAdapterError>;

    /// P2#3: report the adapter's real capability set so `live/open` can
    /// truthfully advertise what the underlying provider supports.
    ///
    /// The default impl returns the no-claim baseline (every capability
    /// `false`). Adapters MUST override this to opt in to the capabilities
    /// they actually support — silence here means "I make no claims", not
    /// "I support everything". Forgetting to override surfaces as a narrow
    /// channel rather than a silently-overstated one.
    fn capabilities(&self) -> LiveChannelCapabilities {
        LiveChannelCapabilities::default()
    }

    /// R5-3: inject a synthetic observation into the same channel the
    /// adapter's pump produces real observations on, so it surfaces
    /// through the next [`Self::next_observation`] read.
    ///
    /// The host calls this when the *runtime* (not the provider) decides
    /// the channel must surface a typed event — the canonical case being
    /// `signal_terminal_error`, where a model-swap on `config/patch`
    /// produces a synthetic `LiveAdapterObservation::Error` that the WS
    /// pump must see *before* the channel is closed. Without this seam
    /// the synthetic event would race against the in-flight
    /// `next_observation()` future and the consumer would observe a
    /// generic close instead of the typed reason.
    ///
    /// Order contract: callers MUST inject before calling [`Self::close`]
    /// so the pump's drain ordering carries the synthetic observation
    /// out ahead of the channel-closed signal.
    ///
    /// The default implementation is a no-op `Ok(())` — adapters that
    /// don't surface typed terminal events through the same channel
    /// (test stubs, future provider adapters that take a different
    /// approach) opt out cleanly. Production adapters that own a real
    /// observation channel MUST override.
    async fn inject_observation(
        &self,
        _observation: LiveAdapterObservation,
    ) -> Result<(), LiveAdapterError> {
        Ok(())
    }
}

/// Errors from the adapter layer. These are transport/provider errors,
/// not Meerkat semantic outcomes (dogma sin #3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LiveAdapterError {
    #[error("adapter not ready: current status is {status:?}")]
    NotReady { status: LiveAdapterStatus },
    #[error("provider error: {message}")]
    ProviderError {
        code: LiveAdapterErrorCode,
        message: String,
    },
    #[error("transport error: {message}")]
    TransportError { message: String },
    #[error("adapter is closed")]
    Closed,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

    use crate::types::{StopReason, Usage};

    // -- Status lifecycle invariants --

    #[test]
    fn idle_is_not_terminal_and_does_not_accept_commands() {
        let status = LiveAdapterStatus::Idle;
        assert!(!status.is_terminal());
        assert!(!status.accepts_commands());
    }

    #[test]
    fn ready_accepts_commands_and_is_not_terminal() {
        let status = LiveAdapterStatus::Ready;
        assert!(!status.is_terminal());
        assert!(status.accepts_commands());
    }

    #[test]
    fn degraded_does_not_accept_commands() {
        let status = LiveAdapterStatus::Degraded {
            reason: LiveDegradationReason::RateLimited,
        };
        assert!(!status.is_terminal());
        assert!(!status.accepts_commands());
    }

    #[test]
    fn closed_is_terminal() {
        let status = LiveAdapterStatus::Closed;
        assert!(status.is_terminal());
        assert!(!status.accepts_commands());
    }

    #[test]
    fn opening_and_closing_are_transient() {
        assert!(!LiveAdapterStatus::Opening.is_terminal());
        assert!(!LiveAdapterStatus::Opening.accepts_commands());
        assert!(!LiveAdapterStatus::Closing.is_terminal());
        assert!(!LiveAdapterStatus::Closing.accepts_commands());
    }

    // -- Degradation reason invariants --

    #[test]
    fn degradation_reason_round_trips_typed_variants() {
        for reason in [
            LiveDegradationReason::RateLimited,
            LiveDegradationReason::ProviderThrottled,
            LiveDegradationReason::NetworkUnstable,
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            let deser: LiveDegradationReason = serde_json::from_str(&json).unwrap();
            assert_eq!(reason, deser);
        }
    }

    #[test]
    fn degradation_reason_other_round_trips_with_static_cow() {
        let reason = LiveDegradationReason::Other {
            detail: Cow::Borrowed("upstream maintenance"),
        };
        let json = serde_json::to_string(&reason).unwrap();
        let deser: LiveDegradationReason = serde_json::from_str(&json).unwrap();
        assert_eq!(reason, deser);
    }

    // -- Command serialization round-trips --

    #[test]
    fn command_interrupt_round_trips() {
        let cmd = LiveAdapterCommand::Interrupt;
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: LiveAdapterCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(deser, LiveAdapterCommand::Interrupt));
    }

    #[test]
    fn command_send_audio_input_round_trips() {
        let cmd = LiveAdapterCommand::SendInput {
            chunk: LiveInputChunk::Audio {
                data: vec![0u8; 480],
                sample_rate_hz: 24000,
                channels: 1,
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        // Must not be a JSON integer array.
        assert!(!json.contains("[0,0,0,0"));
        let deser: LiveAdapterCommand = serde_json::from_str(&json).unwrap();
        match deser {
            LiveAdapterCommand::SendInput {
                chunk:
                    LiveInputChunk::Audio {
                        data,
                        sample_rate_hz,
                        channels,
                    },
            } => {
                assert_eq!(data.len(), 480);
                assert_eq!(sample_rate_hz, 24000);
                assert_eq!(channels, 1);
            }
            other => panic!("expected SendInput/Audio, got {other:?}"),
        }
    }

    #[test]
    fn command_send_text_input_round_trips() {
        let cmd = LiveAdapterCommand::SendInput {
            chunk: LiveInputChunk::Text {
                text: "hello".into(),
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: LiveAdapterCommand = serde_json::from_str(&json).unwrap();
        match deser {
            LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Text { text },
            } => assert_eq!(text, "hello"),
            other => panic!("expected SendInput/Text, got {other:?}"),
        }
    }

    #[test]
    fn command_submit_tool_result_round_trips() {
        let cmd = LiveAdapterCommand::SubmitToolResult {
            result: LiveToolResult {
                call_id: "call_123".into(),
                content: vec![ContentBlock::Text { text: "42".into() }],
                is_error: false,
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: LiveAdapterCommand = serde_json::from_str(&json).unwrap();
        match deser {
            LiveAdapterCommand::SubmitToolResult { result } => {
                assert_eq!(result.call_id, "call_123");
                assert!(!result.is_error);
                assert_eq!(result.content.len(), 1);
                match &result.content[0] {
                    ContentBlock::Text { text } => assert_eq!(text, "42"),
                    other => panic!("expected Text content, got {other:?}"),
                }
            }
            other => panic!("expected SubmitToolResult, got {other:?}"),
        }
    }

    #[test]
    fn command_truncate_round_trips() {
        let cmd = LiveAdapterCommand::TruncateAssistantOutput {
            item_id: "item_abc".into(),
            content_index: 0,
            audio_played_ms: 3200,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: LiveAdapterCommand = serde_json::from_str(&json).unwrap();
        match deser {
            LiveAdapterCommand::TruncateAssistantOutput {
                item_id,
                content_index,
                audio_played_ms,
            } => {
                assert_eq!(item_id, "item_abc");
                assert_eq!(content_index, 0);
                assert_eq!(audio_played_ms, 3200);
            }
            other => panic!("expected TruncateAssistantOutput, got {other:?}"),
        }
    }

    #[test]
    fn command_open_serializes_with_snapshot_version() {
        let cmd = LiveAdapterCommand::Open {
            snapshot: LiveProjectionSnapshot {
                session_id: SessionId::new(),
                snapshot_version: 42,
                seed_messages: vec![],
                visible_tools: vec![],
                system_prompt: Some("You are helpful.".into()),
                model_id: "gpt-5.4".into(),
                provider_id: "openai".into(),
                audio_config: None,
                runtime_system_context: vec![],
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"snapshot_version\":42"));
        assert!(json.contains("\"provider_id\":\"openai\""));
        // R3: empty runtime_system_context must be skipped on the wire so
        // existing consumers don't see a new field they don't recognize.
        assert!(!json.contains("runtime_system_context"));
    }

    #[test]
    fn snapshot_round_trips_with_runtime_system_context() {
        // R3: typed runtime system-context entries must round-trip through
        // the snapshot without being folded into seed_messages.
        use std::time::SystemTime;
        let snapshot = LiveProjectionSnapshot {
            session_id: SessionId::new(),
            snapshot_version: 7,
            seed_messages: vec![],
            visible_tools: vec![],
            system_prompt: None,
            model_id: "gpt-5.4".into(),
            provider_id: "openai".into(),
            audio_config: None,
            runtime_system_context: vec![crate::session::PendingSystemContextAppend {
                text: "peer terminal: pty=42".into(),
                source: Some("peer_terminal".into()),
                idempotency_key: Some("k1".into()),
                accepted_at: SystemTime::UNIX_EPOCH,
            }],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("runtime_system_context"));
        assert!(json.contains("peer_terminal"));
        let deser: LiveProjectionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.runtime_system_context.len(), 1);
        assert_eq!(
            deser.runtime_system_context[0].text,
            "peer terminal: pty=42"
        );
        assert_eq!(
            deser.runtime_system_context[0].source.as_deref(),
            Some("peer_terminal")
        );
    }

    // -- Observation serialization round-trips --

    #[test]
    fn observation_tool_call_requested_round_trips() {
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_456".into(),
            tool_name: "web_search".into(),
            arguments: serde_json::json!({"query": "meerkat habitat"}),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn observation_tool_call_uses_provider_call_id_not_domain_handle() {
        let obs = LiveAdapterObservation::ToolCallRequested {
            provider_call_id: "call_789".into(),
            tool_name: "calculator".into(),
            arguments: serde_json::json!({}),
        };
        if let LiveAdapterObservation::ToolCallRequested {
            provider_call_id, ..
        } = &obs
        {
            assert!(provider_call_id.starts_with("call_"));
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn observation_turn_completed_round_trips() {
        let obs = LiveAdapterObservation::TurnCompleted {
            response_id: None,
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        };
        let json = serde_json::to_string(&obs).unwrap();
        // R6: absent response_id is skipped on the wire.
        assert!(!json.contains("response_id"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn observation_turn_completed_round_trips_with_response_id() {
        // R6: when the provider surfaces a `response.done` with a
        // `response_id`, the observation must carry it so the projection
        // sink can key the buffered final on `(SessionId, response_id)`.
        let obs = LiveAdapterObservation::TurnCompleted {
            response_id: Some("resp_42".into()),
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 12,
                output_tokens: 7,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"response_id\":\"resp_42\""));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn observation_barge_in_is_typed_interrupt_not_side_channel() {
        let obs = LiveAdapterObservation::TurnInterrupted { response_id: None };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("turn_interrupted"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    /// G4 (P1): `TurnInterrupted` carries the in-flight response id so the
    /// projection sink can scope barge-in truncation to the right response
    /// even when the interrupt lands before any transcript delta has been
    /// staged.
    #[test]
    fn observation_turn_interrupted_round_trips_with_response_id() {
        let obs = LiveAdapterObservation::TurnInterrupted {
            response_id: Some("resp_42".into()),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"response_id\":\"resp_42\""));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    // -- Optional provider-item-id variants (N77) --

    #[test]
    fn user_transcript_final_round_trips_with_provider_item_id() {
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item_123".into()),
            previous_item_id: None,
            content_index: None,
            text: "hello".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"provider_item_id\":\"item_123\""));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn user_transcript_final_round_trips_without_provider_item_id() {
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: None,
            previous_item_id: None,
            content_index: None,
            text: "hello".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(
            !json.contains("provider_item_id"),
            "absent field must be skipped on serialize, got {json}"
        );
        assert!(!json.contains("null"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn assistant_text_delta_round_trips_with_and_without_provider_item_id() {
        let with = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: Some("item_xyz".into()),
            previous_item_id: None,
            content_index: None,
            response_id: None,
            delta_id: None,
            delta: "hi".into(),
        };
        let json = serde_json::to_string(&with).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(with, deser);

        let without = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: None,
            previous_item_id: None,
            content_index: None,
            response_id: None,
            delta_id: None,
            delta: "hi".into(),
        };
        let json = serde_json::to_string(&without).unwrap();
        assert!(!json.contains("provider_item_id"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(without, deser);
    }

    // -- T5: AssistantTranscriptDelta is typed-distinct from AssistantTextDelta --

    #[test]
    fn assistant_transcript_delta_round_trips_with_full_ordering_identity() {
        // T5: the spoken-transcript delta variant must serialize with a
        // distinct `observation` tag so producers/consumers cannot collapse
        // it onto the display-text channel.
        let obs = LiveAdapterObservation::AssistantTranscriptDelta {
            provider_item_id: Some("item_t1".into()),
            previous_item_id: Some("item_t0".into()),
            content_index: Some(0),
            response_id: Some("resp_42".into()),
            delta_id: Some("delta_3".into()),
            delta: "spoken".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(
            json.contains("\"observation\":\"assistant_transcript_delta\""),
            "AssistantTranscriptDelta must serialize with its own snake_case tag, got {json}"
        );
        assert!(
            !json.contains("\"observation\":\"assistant_text_delta\""),
            "AssistantTranscriptDelta must NOT collide with assistant_text_delta tag, got {json}"
        );
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn assistant_transcript_delta_round_trips_without_optional_identity() {
        let obs = LiveAdapterObservation::AssistantTranscriptDelta {
            provider_item_id: None,
            previous_item_id: None,
            content_index: None,
            response_id: None,
            delta_id: None,
            delta: "spoken".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(!json.contains("provider_item_id"));
        assert!(!json.contains("response_id"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    // -- A11 ordering identity round-trips --

    #[test]
    fn user_transcript_final_round_trips_with_full_ordering_identity() {
        let obs = LiveAdapterObservation::UserTranscriptFinal {
            provider_item_id: Some("item_123".into()),
            previous_item_id: Some("item_122".into()),
            content_index: Some(0),
            text: "hello".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"previous_item_id\":\"item_122\""));
        assert!(json.contains("\"content_index\":0"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn assistant_text_delta_round_trips_with_full_ordering_identity() {
        let obs = LiveAdapterObservation::AssistantTextDelta {
            provider_item_id: Some("item_xyz".into()),
            previous_item_id: Some("item_xyw".into()),
            content_index: Some(2),
            response_id: Some("resp_42".into()),
            delta_id: Some("delta_7".into()),
            delta: "hi".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"response_id\":\"resp_42\""));
        assert!(json.contains("\"delta_id\":\"delta_7\""));
        assert!(json.contains("\"content_index\":2"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn assistant_transcript_final_round_trips_with_ordering_identity() {
        let obs = LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: "item_abc".into(),
            previous_item_id: Some("item_aba".into()),
            content_index: Some(1),
            response_id: Some("resp_9".into()),
            text: "final transcript".into(),
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"previous_item_id\":\"item_aba\""));
        assert!(json.contains("\"response_id\":\"resp_9\""));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn assistant_transcript_final_round_trips_without_optional_ordering() {
        let obs = LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: "item_only".into(),
            previous_item_id: None,
            content_index: None,
            response_id: None,
            text: "final".into(),
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"provider_item_id\":\"item_only\""));
        assert!(!json.contains("previous_item_id"));
        assert!(!json.contains("content_index"));
        assert!(!json.contains("response_id"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    // -- A12 RealtimeTranscript passthrough --

    #[test]
    fn realtime_transcript_passthrough_round_trips() {
        let inner = RealtimeTranscriptEvent::AssistantTurnInterrupted {
            response_id: "resp_123".into(),
        };
        let obs = LiveAdapterObservation::RealtimeTranscript { event: inner };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"observation\":\"realtime_transcript\""));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn assistant_transcript_truncated_round_trips_all_shapes() {
        let both = LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: Some("item_abc".into()),
            previous_item_id: Some("item_aba".into()),
            content_index: Some(0),
            response_id: Some("resp_5".into()),
            text: Some("partial transcript".into()),
        };
        let json = serde_json::to_string(&both).unwrap();
        assert!(json.contains("\"response_id\":\"resp_5\""));
        assert!(json.contains("\"content_index\":0"));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(both, deser);

        let neither = LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: None,
            previous_item_id: None,
            content_index: None,
            response_id: None,
            text: None,
        };
        let json = serde_json::to_string(&neither).unwrap();
        assert!(!json.contains("provider_item_id"));
        assert!(!json.contains("response_id"));
        assert!(!json.contains("content_index"));
        assert!(!json.contains("\"text\""));
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(neither, deser);

        let id_only = LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: Some("item_def".into()),
            previous_item_id: None,
            content_index: None,
            response_id: None,
            text: None,
        };
        let json = serde_json::to_string(&id_only).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(id_only, deser);

        let text_only = LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: None,
            previous_item_id: None,
            content_index: None,
            response_id: None,
            text: Some("only text".into()),
        };
        let json = serde_json::to_string(&text_only).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(text_only, deser);
    }

    #[test]
    fn observation_status_changed_round_trips() {
        let obs = LiveAdapterObservation::StatusChanged {
            status: LiveAdapterStatus::Degraded {
                reason: LiveDegradationReason::ProviderThrottled,
            },
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn observation_error_round_trips() {
        let obs = LiveAdapterObservation::Error {
            code: LiveAdapterErrorCode::ConnectionLost,
            message: "ws closed unexpectedly".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    /// R5-9: `CommandRejected` is the scoped sibling of `Error`. Distinct
    /// tag on the wire so the host can route command rejections without
    /// closing the channel; round-trip preserves identity.
    #[test]
    fn observation_command_rejected_round_trips_with_distinct_tag() {
        let obs = LiveAdapterObservation::CommandRejected {
            code: LiveAdapterErrorCode::ConfigRejected {
                reason: LiveConfigRejectionReason::ImageInputNotImplemented,
            },
            message: "image_input_not_implemented".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        // Tag must distinguish scoped rejection from terminal Error so the
        // host's classifier can route on the variant, not the message text.
        assert!(json.contains("\"command_rejected\""), "got {json}");
        assert!(!json.contains("\"observation\":\"error\""), "got {json}");
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    // -- Audio chunk binary fidelity --

    #[test]
    fn assistant_audio_chunk_serializes_as_base64_string_not_int_array() {
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x7F, 0xFF];
        let obs = LiveAdapterObservation::AssistantAudioChunk {
            data: bytes.clone(),
            sample_rate_hz: 24000,
            channels: 1,
            response_id: None,
            item_id: None,
            content_index: None,
        };
        let json = serde_json::to_string(&obs).unwrap();
        // Negative assertion: must NOT serialize as a JSON integer array.
        assert!(
            !json.contains("[222,173,"),
            "audio bytes must not serialize as JSON integer array; got {json}"
        );
        // Positive assertion: must contain a base64-encoded data field.
        let expected = BASE64_STANDARD.encode(&bytes);
        assert!(
            json.contains(&format!("\"data\":\"{expected}\"")),
            "expected base64 string for data; got {json}"
        );
        // Round-trip preserves the exact bytes.
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
        match deser {
            LiveAdapterObservation::AssistantAudioChunk { data, .. } => {
                assert_eq!(data, bytes);
            }
            other => panic!("expected AssistantAudioChunk, got {other:?}"),
        }
    }

    /// R5-4: identity fields (`response_id`, `item_id`, `content_index`) on
    /// `AssistantAudioChunk` round-trip cleanly when populated and are
    /// omitted from the wire form when absent.
    #[test]
    fn assistant_audio_chunk_carries_identity_fields_round_trip() {
        let obs = LiveAdapterObservation::AssistantAudioChunk {
            data: vec![1u8, 2, 3],
            sample_rate_hz: 24_000,
            channels: 1,
            response_id: Some("resp_abc".to_string()),
            item_id: Some("item_123".to_string()),
            content_index: Some(0),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("\"response_id\":\"resp_abc\""), "got {json}");
        assert!(json.contains("\"item_id\":\"item_123\""), "got {json}");
        assert!(json.contains("\"content_index\":0"), "got {json}");
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);

        // Skip-when-None: omitting identity must not bloat the wire.
        let obs_anon = LiveAdapterObservation::AssistantAudioChunk {
            data: vec![4u8, 5, 6],
            sample_rate_hz: 24_000,
            channels: 1,
            response_id: None,
            item_id: None,
            content_index: None,
        };
        let json_anon = serde_json::to_string(&obs_anon).unwrap();
        assert!(!json_anon.contains("response_id"), "got {json_anon}");
        assert!(!json_anon.contains("item_id"), "got {json_anon}");
        assert!(!json_anon.contains("content_index"), "got {json_anon}");
        let deser_anon: LiveAdapterObservation = serde_json::from_str(&json_anon).unwrap();
        assert_eq!(obs_anon, deser_anon);
    }

    #[test]
    fn live_input_audio_serializes_as_base64_string() {
        let bytes = vec![1u8, 2, 3, 4, 5];
        let chunk = LiveInputChunk::Audio {
            data: bytes.clone(),
            sample_rate_hz: 24000,
            channels: 1,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(!json.contains("[1,2,3,4,5]"));
        let expected = BASE64_STANDARD.encode(&bytes);
        assert!(json.contains(&expected));
        let deser: LiveInputChunk = serde_json::from_str(&json).unwrap();
        match deser {
            LiveInputChunk::Audio { data, .. } => assert_eq!(data, bytes),
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    // -- Projection snapshot invariants --

    #[test]
    fn snapshot_version_is_monotonic_marker() {
        let s1 = LiveProjectionSnapshot {
            session_id: SessionId::new(),
            snapshot_version: 1,
            seed_messages: vec![],
            visible_tools: vec![],
            system_prompt: None,
            model_id: "gpt-5.4".into(),
            provider_id: "openai".into(),
            audio_config: None,
            runtime_system_context: vec![],
        };
        assert_eq!(s1.snapshot_version, 1);
        let s2 = LiveProjectionSnapshot {
            snapshot_version: 2,
            ..s1
        };
        assert_eq!(s2.snapshot_version, 2);
    }

    // -- Transport bootstrap invariants --

    #[test]
    fn transport_bootstrap_is_tagged_not_bare_url() {
        let ws = LiveTransportBootstrap::Websocket {
            url: "wss://example.com/live".into(),
            token: "tok_abc".into(),
        };
        let json = serde_json::to_string(&ws).unwrap();
        assert!(json.contains("\"transport\":\"websocket\""));
    }

    #[test]
    fn webrtc_bootstrap_is_browser_offer_signaling_not_meerkat_offer() {
        let webrtc = LiveTransportBootstrap::Webrtc {
            token: "tok_webrtc".into(),
            answer_method: "live/webrtc/answer".into(),
            http_url: None,
        };
        let json = serde_json::to_value(&webrtc).unwrap();
        assert_eq!(json["transport"], "webrtc");
        assert_eq!(json["answer_method"], "live/webrtc/answer");
        assert!(
            json.get("offer_sdp").is_none(),
            "Meerkat must not pre-compute browser-offer SDP"
        );
    }

    // -- Continuity mode invariants --

    #[test]
    fn continuity_mode_distinguishes_provider_native_from_transcript_only() {
        let resume = LiveContinuityMode::ProviderNativeResume {
            provider_session_id: "sess_abc".into(),
        };
        assert_ne!(resume, LiveContinuityMode::TranscriptOnly);
        assert_ne!(
            LiveContinuityMode::TranscriptOnly,
            LiveContinuityMode::Degraded
        );
        assert_ne!(LiveContinuityMode::Degraded, LiveContinuityMode::Fresh);
    }

    // -- T12: ProviderNativeResume round-trip --

    #[test]
    fn continuity_mode_provider_native_resume_round_trips() {
        // T12: the resume variant must carry the provider session id and
        // round-trip through serde with the snake_case `mode` tag.
        let mode = LiveContinuityMode::ProviderNativeResume {
            provider_session_id: "sess_42".to_string(),
        };
        let json = serde_json::to_string(&mode).unwrap();
        assert!(
            json.contains("\"mode\":\"provider_native_resume\""),
            "ProviderNativeResume must serialize with the snake_case mode tag, got {json}"
        );
        assert!(
            json.contains("\"provider_session_id\":\"sess_42\""),
            "ProviderNativeResume must carry provider_session_id verbatim, got {json}"
        );
        let deser: LiveContinuityMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, deser);
    }

    #[test]
    fn continuity_mode_payload_less_variants_round_trip() {
        for mode in [
            LiveContinuityMode::Fresh,
            LiveContinuityMode::TranscriptOnly,
            LiveContinuityMode::Degraded,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let deser: LiveContinuityMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deser);
        }
    }

    // -- T11: Image / VideoFrame input chunks --

    #[test]
    fn live_input_image_round_trips_with_base64_bytes() {
        let bytes = vec![0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let chunk = LiveInputChunk::Image {
            mime: "image/png".to_string(),
            data: bytes.clone(),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        // Negative: must NOT serialize bytes as a JSON integer array.
        assert!(
            !json.contains("[137,80,78"),
            "image bytes must not serialize as JSON integer array; got {json}"
        );
        // Positive: base64 wrapper mirrors AssistantAudioChunk / Audio.
        let expected = BASE64_STANDARD.encode(&bytes);
        assert!(
            json.contains(&format!("\"data\":\"{expected}\"")),
            "expected base64 string for image data; got {json}"
        );
        assert!(json.contains("\"kind\":\"image\""));
        assert!(json.contains("\"mime\":\"image/png\""));
        let deser: LiveInputChunk = serde_json::from_str(&json).unwrap();
        match deser {
            LiveInputChunk::Image { mime, data } => {
                assert_eq!(mime, "image/png");
                assert_eq!(data, bytes);
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn live_input_video_frame_round_trips_with_codec_and_timestamp() {
        let bytes = vec![0u8, 1, 2, 3, 4, 5, 6, 7];
        let chunk = LiveInputChunk::VideoFrame {
            codec: "vp8".to_string(),
            data: bytes.clone(),
            timestamp_ms: 12_345,
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"kind\":\"video_frame\""));
        assert!(json.contains("\"codec\":\"vp8\""));
        assert!(json.contains("\"timestamp_ms\":12345"));
        let expected = BASE64_STANDARD.encode(&bytes);
        assert!(json.contains(&format!("\"data\":\"{expected}\"")));
        let deser: LiveInputChunk = serde_json::from_str(&json).unwrap();
        match deser {
            LiveInputChunk::VideoFrame {
                codec,
                data,
                timestamp_ms,
            } => {
                assert_eq!(codec, "vp8");
                assert_eq!(data, bytes);
                assert_eq!(timestamp_ms, 12_345);
            }
            other => panic!("expected VideoFrame, got {other:?}"),
        }
    }

    #[test]
    fn channel_open_response_includes_continuity() {
        let resp = LiveChannelOpenResponse {
            transport: LiveTransportBootstrap::Websocket {
                url: "wss://example.com/live".into(),
                token: "tok_abc".into(),
            },
            input_audio_format: Some(LiveAudioConfig {
                input_sample_rate_hz: 24000,
                input_channels: 1,
                output_sample_rate_hz: 24000,
                output_channels: 1,
            }),
            // Explicit capability set: no-claim baseline is `false` for
            // every field, so the round-trip needs explicit `true`s to
            // exercise the on-the-wire shape meaningfully.
            capabilities: LiveChannelCapabilities {
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
            continuity: LiveContinuityMode::TranscriptOnly,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deser: LiveChannelOpenResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, deser);
        assert_eq!(deser.continuity, LiveContinuityMode::TranscriptOnly);
        assert!(!deser.capabilities.provider_native_resume);
        assert!(deser.capabilities.audio_in);
        assert!(deser.capabilities.audio_out);
    }

    // -- Capabilities default invariants --

    #[test]
    fn default_capabilities_make_no_claims() {
        // No-claim baseline: every capability defaults to `false`. Adapters
        // must opt in explicitly — silence here means "no claim made", not
        // "support everything".
        let caps = LiveChannelCapabilities::default();
        assert!(!caps.audio_in);
        assert!(!caps.audio_out);
        assert!(!caps.text_in);
        assert!(!caps.text_out);
        assert!(!caps.barge_in_supported);
        assert!(!caps.transcript_supported);
        assert!(!caps.image_in);
        assert!(!caps.video_in);
        assert!(!caps.provider_native_resume);
    }

    #[test]
    fn capabilities_carry_typed_image_and_video_booleans() {
        // T8: gpt-realtime-2 / Gemini Live capability shape — both can be
        // expressed without provider-specific fields.
        let gpt_realtime_2 = LiveChannelCapabilities {
            image_in: true,
            ..LiveChannelCapabilities::default()
        };
        let gemini_live = LiveChannelCapabilities {
            video_in: true,
            ..LiveChannelCapabilities::default()
        };
        assert!(gpt_realtime_2.image_in);
        assert!(!gpt_realtime_2.video_in);
        assert!(gemini_live.video_in);
        assert!(!gemini_live.image_in);
    }

    // -- Error code invariants --

    #[test]
    fn adapter_error_codes_round_trip() {
        for code in [
            LiveAdapterErrorCode::ConnectionFailed,
            LiveAdapterErrorCode::ConnectionLost,
            LiveAdapterErrorCode::ProviderError,
            LiveAdapterErrorCode::AuthenticationFailed,
            LiveAdapterErrorCode::InternalError,
        ] {
            let json = serde_json::to_string(&code).unwrap();
            let deser: LiveAdapterErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(code, deser);
        }
    }

    #[test]
    fn adapter_error_code_config_rejected_round_trips() {
        // R12 + R5-2: ConfigRejected carries a typed
        // `LiveConfigRejectionReason` and routes distinctly from
        // ProviderError so callers can act on the typed code (and now the
        // typed reason variant) without parsing English from the message
        // field.
        let code = LiveAdapterErrorCode::ConfigRejected {
            reason: LiveConfigRejectionReason::RefreshModelSwap {
                from_model: "gpt-realtime".to_string(),
                to_model: "gpt-realtime-mini-v2".to_string(),
            },
        };
        let json = serde_json::to_string(&code).unwrap();
        assert!(
            json.contains("\"code\":\"config_rejected\""),
            "ConfigRejected must serialize with the snake_case tag, got {json}"
        );
        assert!(
            json.contains("\"kind\":\"refresh_model_swap\""),
            "reason must serialize with its typed kind discriminator, got {json}"
        );
        let deser: LiveAdapterErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(code, deser);
        match deser {
            LiveAdapterErrorCode::ConfigRejected { reason } => {
                assert!(matches!(
                    reason,
                    LiveConfigRejectionReason::RefreshModelSwap { ref to_model, .. }
                        if to_model == "gpt-realtime-mini-v2"
                ));
                // Display impl preserves the human-readable swap text used
                // by `signal_terminal_error` to populate `Error.message`.
                assert!(format!("{reason}").contains("close + reopen"));
            }
            other => panic!("expected ConfigRejected, got {other:?}"),
        }
    }

    /// R5-2 (P2 dogma): every typed `LiveConfigRejectionReason` variant
    /// round-trips through JSON byte-identical and preserves variant
    /// identity after deserialization. This is the structural gate that
    /// pins the wire-visible discriminator (`kind`) on every variant.
    #[test]
    fn config_rejection_reason_round_trips_each_typed_variant() {
        let cases = vec![
            LiveConfigRejectionReason::ChannelIdentitySwap {
                from_model: "gpt-realtime".into(),
                from_provider: Provider::OpenAI,
                to_model: "gpt-realtime-2".into(),
                to_provider: Provider::OpenAI,
            },
            LiveConfigRejectionReason::NonRealtimeResolution {
                detail: "ModelNotRealtime { model: \"gpt-5.4\", provider: \"openai\" }".into(),
            },
            LiveConfigRejectionReason::ImageInputNotImplemented,
            LiveConfigRejectionReason::VideoFrameInputNotImplemented,
            LiveConfigRejectionReason::UnsupportedInputChunkVariant,
            LiveConfigRejectionReason::RefreshModelSwap {
                from_model: "gpt-realtime".into(),
                to_model: "gpt-realtime-2".into(),
            },
            LiveConfigRejectionReason::RefreshProviderSwap {
                from_provider: "openai".into(),
                to_provider: "anthropic".into(),
            },
            LiveConfigRejectionReason::RefreshAudioConfigMismatch {
                detail: "rate=16000/16000 ch=1/1 cannot be applied in place".into(),
            },
            LiveConfigRejectionReason::AudioInputFormatMismatch {
                expected_sample_rate_hz: 24_000,
                expected_channels: 1,
                actual_sample_rate_hz: 16_000,
                actual_channels: 2,
            },
            LiveConfigRejectionReason::Other {
                detail: "diagnostic catch-all".into(),
            },
        ];
        for case in cases {
            let json = serde_json::to_string(&case).unwrap();
            assert!(
                json.contains("\"kind\":\""),
                "reason must carry a `kind` discriminator, got {json}"
            );
            let back: LiveConfigRejectionReason = serde_json::from_str(&json).unwrap();
            assert_eq!(case, back, "round-trip must preserve identity");
            // Display never panics and always produces non-empty text.
            assert!(!format!("{case}").is_empty());
        }
    }

    #[test]
    fn adapter_error_code_other_preserves_raw_value() {
        let code = LiveAdapterErrorCode::Other {
            raw: "provider_specific_42".into(),
        };
        let json = serde_json::to_string(&code).unwrap();
        let deser: LiveAdapterErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(code, deser);
        match deser {
            LiveAdapterErrorCode::Other { raw } => assert_eq!(raw, "provider_specific_42"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    // -- LiveAdapterError invariants --

    #[test]
    fn adapter_error_not_ready_carries_status() {
        let err = LiveAdapterError::NotReady {
            status: LiveAdapterStatus::Opening,
        };
        let msg = err.to_string();
        assert!(msg.contains("Opening"));
    }

    #[test]
    fn adapter_error_closed_is_distinct_from_not_ready() {
        let closed = LiveAdapterError::Closed;
        let not_ready = LiveAdapterError::NotReady {
            status: LiveAdapterStatus::Closed,
        };
        assert_ne!(closed, not_ready);
    }

    // -- LiveToolResult invariants --

    #[test]
    fn live_tool_result_round_trips() {
        let result = LiveToolResult {
            call_id: "call_abc".into(),
            content: vec![ContentBlock::Text {
                text: "answer is 42".into(),
            }],
            is_error: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: LiveToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, deser);
    }

    #[test]
    fn live_tool_result_error_flag_round_trips() {
        let result = LiveToolResult {
            call_id: "call_err".into(),
            content: vec![ContentBlock::Text {
                text: "tool not found".into(),
            }],
            is_error: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: LiveToolResult = serde_json::from_str(&json).unwrap();
        assert!(deser.is_error);
    }

    #[test]
    fn live_tool_result_preserves_multiple_content_blocks() {
        let result = LiveToolResult {
            call_id: "call_multi".into(),
            content: vec![
                ContentBlock::Text {
                    text: "first".into(),
                },
                ContentBlock::Text {
                    text: "second".into(),
                },
            ],
            is_error: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: LiveToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.content.len(), 2);
        assert_eq!(result, deser);
    }

    // -- JsonSchema smoke pin --
    //
    // Pins the schema-emission path so a future schemars or wire-type bump
    // can't silently break SDK codegen / `meerkat-contracts` schema export.

    #[cfg(feature = "schema")]
    #[test]
    fn live_adapter_observation_emits_round_trippable_schema() {
        let schema = schemars::schema_for!(LiveAdapterObservation);
        let json = serde_json::to_string(&schema).unwrap();
        let _deser: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json.contains("LiveAdapterObservation"));
    }

    #[cfg(feature = "schema")]
    #[test]
    fn live_adapter_command_emits_round_trippable_schema() {
        let schema = schemars::schema_for!(LiveAdapterCommand);
        let json = serde_json::to_string(&schema).unwrap();
        let _deser: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json.contains("LiveAdapterCommand"));
    }

    #[cfg(feature = "schema")]
    #[test]
    fn live_channel_open_response_emits_round_trippable_schema() {
        let schema = schemars::schema_for!(LiveChannelOpenResponse);
        let json = serde_json::to_string(&schema).unwrap();
        let _deser: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json.contains("LiveChannelOpenResponse"));
    }
}
