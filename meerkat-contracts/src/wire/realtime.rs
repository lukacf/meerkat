//! Provider-realtime-session shared shapes.
//!
//! These types describe the OpenAI Realtime API vocabulary (and any future
//! provider-realtime equivalent) — chunk shapes, format descriptors, and
//! capability projections that the live adapter implementations
//! (`meerkat-openai`, `meerkat-live`) speak in. The deleted RPC `realtime/*`
//! channel-framing surface is gone (live-adapter MVP); only the
//! provider-session vocabulary remains. Keep "Realtime" naming distinct from
//! "Live" naming: `Live*` types describe Meerkat's adapter abstraction;
//! `Realtime*` types describe the provider session vocabulary the adapter
//! wraps.

use serde::{Deserialize, Serialize};

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

/// Turning mode for a provider realtime session.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTurningMode {
    ProviderManaged,
    ExplicitCommit,
}

/// Input modality kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeInputKind {
    Text,
    Audio,
    Video,
}

/// Output modality kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeOutputKind {
    Text,
    Audio,
    Video,
}

/// Provider-session capability projection used by the live adapter to
/// publish what the negotiated provider session can actually do.
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

/// A text chunk for realtime ingress/egress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeTextChunk {
    pub text: String,
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

/// Modality-neutral provider input chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeInputChunk {
    TextChunk(RealtimeTextChunk),
    AudioChunk(RealtimeAudioChunk),
    VideoChunk(RealtimeVideoChunk),
}
