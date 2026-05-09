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

use crate::realtime_transcript::RealtimeTranscriptEvent;
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
    Open {
        snapshot: LiveProjectionSnapshot,
    },
    /// P1#5: refresh an *already-open* adapter's projection with a freshly
    /// built [`LiveProjectionSnapshot`].
    ///
    /// Triggered when upstream session state changes (model switch via
    /// `config/patch`, snapshot drift after a session edit, etc.) and the
    /// runtime wants the live adapter to re-seed its provider session
    /// against the new canonical state without tearing the channel down.
    /// Adapters that do not support live re-seeding should treat this as a
    /// no-op or surface a typed error observation; the OpenAI adapter
    /// re-runs `seed_history_projection` against the new snapshot.
    Refresh {
        snapshot: LiveProjectionSnapshot,
    },
    SendInput {
        chunk: LiveInputChunk,
    },
    CommitInput,
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
    AssistantAudioChunk {
        #[serde(with = "base64_bytes")]
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        data: Vec<u8>,
        sample_rate_hz: u32,
        channels: u16,
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
    TurnInterrupted,
    TurnCompleted {
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
    ConfigurationRejected,
    ProviderError,
    AuthenticationFailed,
    InternalError,
    /// Unknown provider error code preserved verbatim for diagnostics.
    Other {
        raw: String,
    },
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
}

// ---------------------------------------------------------------------------
// Transport bootstrap — tagged transport info for surface API
// ---------------------------------------------------------------------------

/// A single ICE server entry for WebRTC bootstrap. Replaces a bare
/// `Vec<String>` so credentialed TURN servers carry their auth fields
/// alongside their URLs (typed truth, dogma sin #1).
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveIceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// Tagged transport bootstrap returned when opening a live channel.
///
/// The surface API returns this instead of a bare `ws_url` so Meerkat
/// semantics do not depend on transport kind (dogma sin #10).
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
        offer_sdp: String,
        ice_servers: Vec<LiveIceServer>,
    },
}

/// Capabilities advertised when a live channel opens.
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveChannelCapabilities {
    pub audio_input: bool,
    pub audio_output: bool,
    pub text_input: bool,
    pub text_output: bool,
    pub barge_in: bool,
    pub transcript: bool,
    pub provider_native_resume: bool,
}

impl Default for LiveChannelCapabilities {
    fn default() -> Self {
        Self {
            audio_input: true,
            audio_output: true,
            text_input: true,
            text_output: true,
            barge_in: true,
            transcript: true,
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
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveContinuityMode {
    ProviderNative,
    TranscriptOnly,
    Degraded,
    Fresh,
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
    /// The default impl returns the conservative-but-honest baseline used by
    /// every realtime provider Meerkat ships today (text + audio in/out,
    /// barge-in, transcripts; no provider-native resume yet). Providers that
    /// expose narrower or richer capability sets should override this.
    fn capabilities(&self) -> LiveChannelCapabilities {
        LiveChannelCapabilities {
            audio_input: true,
            audio_output: true,
            text_input: true,
            text_output: true,
            barge_in: true,
            transcript: true,
            provider_native_resume: false,
        }
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
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"snapshot_version\":42"));
        assert!(json.contains("\"provider_id\":\"openai\""));
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
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deser: LiveAdapterObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(obs, deser);
    }

    #[test]
    fn observation_barge_in_is_typed_interrupt_not_side_channel() {
        let obs = LiveAdapterObservation::TurnInterrupted;
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("turn_interrupted"));
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

    // -- Audio chunk binary fidelity --

    #[test]
    fn assistant_audio_chunk_serializes_as_base64_string_not_int_array() {
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x7F, 0xFF];
        let obs = LiveAdapterObservation::AssistantAudioChunk {
            data: bytes.clone(),
            sample_rate_hz: 24000,
            channels: 1,
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

        let webrtc = LiveTransportBootstrap::Webrtc {
            offer_sdp: "v=0\r\n...".into(),
            ice_servers: vec![LiveIceServer {
                urls: vec!["stun:stun.example.com".into()],
                username: None,
                credential: None,
            }],
        };
        let json = serde_json::to_string(&webrtc).unwrap();
        assert!(json.contains("\"transport\":\"webrtc\""));
    }

    #[test]
    fn ice_server_round_trips_with_credentials() {
        let server = LiveIceServer {
            urls: vec![
                "turn:turn.example.com:3478".into(),
                "turns:turn.example.com:5349".into(),
            ],
            username: Some("user".into()),
            credential: Some("secret".into()),
        };
        let json = serde_json::to_string(&server).unwrap();
        assert!(json.contains("\"username\":\"user\""));
        assert!(json.contains("\"credential\":\"secret\""));
        let deser: LiveIceServer = serde_json::from_str(&json).unwrap();
        assert_eq!(server, deser);
    }

    #[test]
    fn ice_server_omits_optional_fields_when_absent() {
        let server = LiveIceServer {
            urls: vec!["stun:stun.example.com".into()],
            username: None,
            credential: None,
        };
        let json = serde_json::to_string(&server).unwrap();
        assert!(!json.contains("username"));
        assert!(!json.contains("credential"));
        let deser: LiveIceServer = serde_json::from_str(&json).unwrap();
        assert_eq!(server, deser);
    }

    #[test]
    fn webrtc_bootstrap_carries_typed_ice_servers() {
        let webrtc = LiveTransportBootstrap::Webrtc {
            offer_sdp: "v=0\r\n...".into(),
            ice_servers: vec![LiveIceServer {
                urls: vec!["turn:turn.example.com".into()],
                username: Some("u".into()),
                credential: Some("c".into()),
            }],
        };
        let json = serde_json::to_string(&webrtc).unwrap();
        let deser: LiveTransportBootstrap = serde_json::from_str(&json).unwrap();
        assert_eq!(webrtc, deser);
    }

    // -- Continuity mode invariants --

    #[test]
    fn continuity_mode_distinguishes_provider_native_from_transcript_only() {
        assert_ne!(
            LiveContinuityMode::ProviderNative,
            LiveContinuityMode::TranscriptOnly
        );
        assert_ne!(
            LiveContinuityMode::TranscriptOnly,
            LiveContinuityMode::Degraded
        );
        assert_ne!(LiveContinuityMode::Degraded, LiveContinuityMode::Fresh);
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
            capabilities: LiveChannelCapabilities::default(),
            continuity: LiveContinuityMode::TranscriptOnly,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deser: LiveChannelOpenResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, deser);
        assert_eq!(deser.continuity, LiveContinuityMode::TranscriptOnly);
        assert!(!deser.capabilities.provider_native_resume);
    }

    // -- Capabilities default invariants --

    #[test]
    fn default_capabilities_enable_core_features_but_not_provider_resume() {
        let caps = LiveChannelCapabilities::default();
        assert!(caps.audio_input);
        assert!(caps.audio_output);
        assert!(caps.barge_in);
        assert!(caps.transcript);
        assert!(!caps.provider_native_resume);
    }

    // -- Error code invariants --

    #[test]
    fn adapter_error_codes_round_trip() {
        for code in [
            LiveAdapterErrorCode::ConnectionFailed,
            LiveAdapterErrorCode::ConnectionLost,
            LiveAdapterErrorCode::ConfigurationRejected,
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
