//! Live adapter seam vocabulary.
//!
//! Defines the typed command/observation boundary between Meerkat runtime
//! (which owns semantic authority) and live provider adapters (which own
//! provider transport mechanics). This is the single language both sides
//! speak — no provider-specific types cross this boundary.

use serde::{Deserialize, Serialize};

use crate::types::{Message, SessionId, StopReason, ToolDef, Usage};

// ---------------------------------------------------------------------------
// Adapter status — typed lifecycle, not Option<T> hiding five states
// ---------------------------------------------------------------------------

/// Typed lifecycle phase of a live adapter session.
///
/// Replaces `Option<provider_session>` patterns where `None` means five
/// different things (dogma sin #7). Each variant is a distinct semantic
/// state the adapter host can act on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterStatus {
    Idle,
    Opening,
    Ready,
    Degraded { reason: String },
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

// ---------------------------------------------------------------------------
// Seam-owned tool result — adapter doesn't need ContentBlock knowledge
// ---------------------------------------------------------------------------

/// Tool result at the adapter seam. Uses JSON content instead of
/// `Vec<ContentBlock>` because the adapter only needs to ferry the result
/// back to the provider — it doesn't interpret content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveToolResult {
    pub call_id: String,
    pub content: serde_json::Value,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterCommand {
    Open {
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterObservation {
    Ready,
    UserTranscriptFinal {
        provider_item_id: String,
        text: String,
    },
    AssistantTextDelta {
        provider_item_id: String,
        delta: String,
    },
    AssistantAudioChunk {
        data: Vec<u8>,
        sample_rate_hz: u32,
        channels: u16,
    },
    AssistantTranscriptFinal {
        provider_item_id: String,
        text: String,
        stop_reason: StopReason,
        usage: Usage,
    },
    AssistantTranscriptTruncated {
        provider_item_id: String,
        text: String,
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

/// Typed error codes for adapter-level failures. Transport/provider errors
/// are not Meerkat terminal outcomes (dogma sin #3) — the runtime decides
/// the semantic consequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveAdapterErrorCode {
    ConnectionFailed,
    ConnectionLost,
    ConfigurationRejected,
    ProviderError,
    AuthenticationFailed,
    InternalError,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveProjectionSnapshot {
    pub session_id: SessionId,
    pub snapshot_version: u64,
    pub seed_messages: Vec<Message>,
    pub visible_tools: Vec<ToolDef>,
    pub system_prompt: Option<String>,
    pub model_id: String,
    pub provider_id: String,
    pub audio_config: Option<LiveAudioConfig>,
}

/// Audio format configuration for the live session.
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LiveInputChunk {
    Audio {
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

/// Tagged transport bootstrap returned when opening a live channel.
///
/// The surface API returns this instead of a bare `ws_url` so Meerkat
/// semantics do not depend on transport kind (dogma sin #10).
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
        ice_servers: Vec<String>,
    },
}

/// Capabilities advertised when a live channel opens.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveChannelOpenResponse {
    pub transport: LiveTransportBootstrap,
    pub input_audio_format: Option<LiveAudioConfig>,
    pub capabilities: LiveChannelCapabilities,
    pub continuity: LiveContinuityMode,
}

/// Explicit continuity classification (dogma sin #8: no resume lies).
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
pub trait LiveAdapter: Send {
    async fn send_command(&mut self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError>;

    async fn next_observation(
        &mut self,
    ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError>;

    fn status(&self) -> &LiveAdapterStatus;

    async fn close(&mut self) -> Result<(), LiveAdapterError>;
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
            reason: "rate limited".into(),
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
                content: serde_json::json!({"result": "42"}),
                is_error: false,
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: LiveAdapterCommand = serde_json::from_str(&json).unwrap();
        match deser {
            LiveAdapterCommand::SubmitToolResult { result } => {
                assert_eq!(result.call_id, "call_123");
                assert!(!result.is_error);
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

    #[test]
    fn observation_status_changed_round_trips() {
        let obs = LiveAdapterObservation::StatusChanged {
            status: LiveAdapterStatus::Degraded {
                reason: "provider throttled".into(),
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
            ice_servers: vec!["stun:stun.example.com".into()],
        };
        let json = serde_json::to_string(&webrtc).unwrap();
        assert!(json.contains("\"transport\":\"webrtc\""));
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
            content: serde_json::json!({"answer": 42}),
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
            content: serde_json::json!("tool not found"),
            is_error: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: LiveToolResult = serde_json::from_str(&json).unwrap();
        assert!(deser.is_error);
    }
}
