//! Internal realtime adapter seam.

use async_trait::async_trait;
use meerkat_contracts::{
    RealtimeAudioChunk, RealtimeAudioFormat, RealtimeCapabilities, RealtimeInputChunk,
    RealtimeTurningMode, RealtimeVideoChunk,
};
use meerkat_core::{
    PendingSystemContextAppend, RealtimeTranscriptEvent, SessionLlmIdentity, StopReason, ToolDef,
    ToolResult,
    types::{Message, Usage},
};
use serde_json::Value;

use crate::LlmError;
use crate::realtime_session::{RealtimeSessionEvent, RealtimeSessionOpenConfig};

/// Adapter generation minted by the host every time it opens or rebuilds a
/// provider session. Hosts fence observations with this value before admitting
/// them into Meerkat-owned control paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealtimeAdapterGeneration(pub u64);

impl RealtimeAdapterGeneration {
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Wrapped provider item identifier. This is an internal diagnostic/provider
/// reference, not a public Meerkat control handle.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealtimeProviderItemRef(String);

impl RealtimeProviderItemRef {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for RealtimeProviderItemRef {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Wrapped provider response identifier. This remains adapter-owned state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealtimeProviderResponseRef(String);

impl RealtimeProviderResponseRef {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for RealtimeProviderResponseRef {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Wrapped provider tool-call identifier. Runtime tool authority still lives
/// outside the adapter; this value only lets the adapter correlate provider
/// requests and provider result submission.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealtimeProviderToolCallRef(String);

impl RealtimeProviderToolCallRef {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for RealtimeProviderToolCallRef {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Continuity quality used when opening or rebuilding a provider session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RealtimeAdapterContinuity {
    /// Provider-native resume preserved provider-side session continuity.
    ProviderNative,
    /// Provider session was rebuilt from canonical Meerkat transcript.
    TranscriptOnly,
    /// Rebuild was possible, but some projection facts were unavailable or
    /// intentionally omitted. This must not be reported as full restoration.
    Degraded,
}

/// Explicit projection version/frontier/digest. This is evidence for rebuild
/// and stale-observation fencing; it is not semantic authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RealtimeProjectionWatermark {
    pub version: u64,
    pub frontier: u64,
    pub digest: String,
}

/// Canonical Meerkat state projected into a provider session.
#[derive(Debug, Clone)]
pub struct RealtimeProjectionSnapshot {
    pub identity: SessionLlmIdentity,
    pub turning_mode: RealtimeTurningMode,
    pub visible_tools: Vec<ToolDef>,
    pub seed_messages: Vec<Message>,
    pub runtime_system_context: Vec<PendingSystemContextAppend>,
    pub watermark: RealtimeProjectionWatermark,
    pub continuity: RealtimeAdapterContinuity,
    pub response_nudge_timeout_ms: Option<u64>,
    pub response_nudge_max_attempts: Option<u8>,
}

impl RealtimeProjectionSnapshot {
    /// Bridge to the existing provider open config while the OpenAI adapter is
    /// ported. The watermark and continuity stay on the seam; OpenAI receives
    /// only the projection mechanics it already understands.
    #[must_use]
    pub fn to_open_config(&self) -> RealtimeSessionOpenConfig {
        let mut open_config = RealtimeSessionOpenConfig::new(
            self.turning_mode,
            self.identity.clone(),
            self.visible_tools.clone(),
            self.seed_messages.clone(),
        );
        open_config.runtime_system_context = self.runtime_system_context.clone();
        open_config.response_nudge_timeout_ms = self.response_nudge_timeout_ms;
        open_config.response_nudge_max_attempts = self.response_nudge_max_attempts;
        open_config
    }
}

/// Provider adapter lifecycle status. Transport/provider status is reported as
/// adapter truth only; callers must map it into Meerkat semantics explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeAdapterStatus {
    NotOpened,
    Opening,
    Ready,
    Degraded { reason: String },
    Closing,
    Closed,
    Terminal { reason: String },
}

impl RealtimeAdapterStatus {
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Opening | Self::Ready | Self::Degraded { .. })
    }
}

/// Commands from the runtime-owned live host into a provider adapter.
#[derive(Debug, Clone)]
pub enum RealtimeAdapterCommand {
    OpenSession {
        snapshot: RealtimeProjectionSnapshot,
    },
    RebuildSession {
        snapshot: RealtimeProjectionSnapshot,
        reason: String,
    },
    SendAdmittedInput {
        chunk: RealtimeInputChunk,
    },
    CommitInput,
    Interrupt {
        reason: String,
    },
    Steer {
        instruction: String,
    },
    TruncateAssistantOutput {
        item: RealtimeProviderItemRef,
        content_index: u32,
        audio_played_ms: u64,
    },
    SubmitToolResult {
        result: ToolResult,
    },
    SubmitToolError {
        call: RealtimeProviderToolCallRef,
        error: String,
    },
    CloseSession {
        reason: String,
    },
}

/// Provider-originated assistant output normalized at the adapter seam.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeAdapterAssistantOutput {
    TextDelta {
        response: Option<RealtimeProviderResponseRef>,
        delta_id: Option<String>,
        item: Option<RealtimeProviderItemRef>,
        previous_item: Option<RealtimeProviderItemRef>,
        content_index: Option<u32>,
        delta: String,
    },
    AudioChunk {
        chunk: RealtimeAudioChunk,
    },
    VideoChunk {
        chunk: RealtimeVideoChunk,
    },
}

/// Typed observation emitted by provider adapters. Observations are facts for
/// the live host to gate into canonical Meerkat APIs; they do not by
/// themselves mutate runtime semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeAdapterObservation {
    ProviderStatus {
        status: RealtimeAdapterStatus,
    },
    ProviderTranscriptPartial {
        text: String,
    },
    ProviderTranscriptFinal {
        item: RealtimeProviderItemRef,
        previous_item: Option<RealtimeProviderItemRef>,
        content_index: u32,
        text: String,
    },
    ProviderTranscriptFinalText {
        text: String,
    },
    ProviderTranscriptEvent {
        event: RealtimeTranscriptEvent,
    },
    ProviderInputTurnStarted,
    ProviderInputCommitted,
    ProviderAssistantOutput {
        output: RealtimeAdapterAssistantOutput,
    },
    ProviderAssistantOutputTruncated {
        response: Option<RealtimeProviderResponseRef>,
        item: RealtimeProviderItemRef,
        audio_played_ms: u64,
        truncated_text: Option<String>,
    },
    ProviderAssistantTurnCompleted {
        response: RealtimeProviderResponseRef,
        stop_reason: StopReason,
        usage: Usage,
    },
    InterruptRequested {
        response: Option<RealtimeProviderResponseRef>,
    },
    ProviderToolCallRequested {
        call: RealtimeProviderToolCallRef,
        name: String,
        arguments: Value,
    },
}

impl From<RealtimeSessionEvent> for RealtimeAdapterObservation {
    fn from(event: RealtimeSessionEvent) -> Self {
        match event {
            RealtimeSessionEvent::InputTranscriptPartial { text } => {
                Self::ProviderTranscriptPartial { text }
            }
            RealtimeSessionEvent::InputTranscriptFinal { text } => {
                Self::ProviderTranscriptFinalText { text }
            }
            RealtimeSessionEvent::InputTranscriptFinalForItem {
                item_id,
                previous_item_id,
                content_index,
                text,
            } => Self::ProviderTranscriptFinal {
                item: RealtimeProviderItemRef::new(item_id),
                previous_item: previous_item_id.map(RealtimeProviderItemRef::new),
                content_index,
                text,
            },
            RealtimeSessionEvent::TurnStarted => Self::ProviderInputTurnStarted,
            RealtimeSessionEvent::TurnCommitted => Self::ProviderInputCommitted,
            RealtimeSessionEvent::TurnCompleted {
                response_id,
                stop_reason,
                usage,
            } => Self::ProviderAssistantTurnCompleted {
                response: RealtimeProviderResponseRef::new(response_id),
                stop_reason,
                usage,
            },
            RealtimeSessionEvent::OutputTextDelta { delta } => Self::ProviderAssistantOutput {
                output: RealtimeAdapterAssistantOutput::TextDelta {
                    response: None,
                    delta_id: None,
                    item: None,
                    previous_item: None,
                    content_index: None,
                    delta,
                },
            },
            RealtimeSessionEvent::OutputTextDeltaForItem {
                response_id,
                delta_id,
                item_id,
                previous_item_id,
                content_index,
                delta,
            } => Self::ProviderAssistantOutput {
                output: RealtimeAdapterAssistantOutput::TextDelta {
                    response: Some(RealtimeProviderResponseRef::new(response_id)),
                    delta_id: Some(delta_id),
                    item: Some(RealtimeProviderItemRef::new(item_id)),
                    previous_item: previous_item_id.map(RealtimeProviderItemRef::new),
                    content_index: Some(content_index),
                    delta,
                },
            },
            RealtimeSessionEvent::OutputAudioChunk { chunk } => Self::ProviderAssistantOutput {
                output: RealtimeAdapterAssistantOutput::AudioChunk { chunk },
            },
            RealtimeSessionEvent::OutputVideoChunk { chunk } => Self::ProviderAssistantOutput {
                output: RealtimeAdapterAssistantOutput::VideoChunk { chunk },
            },
            RealtimeSessionEvent::Interrupted { response_id } => Self::InterruptRequested {
                response: response_id.map(RealtimeProviderResponseRef::new),
            },
            RealtimeSessionEvent::ToolCallRequested {
                call_id,
                tool_name,
                arguments,
            } => Self::ProviderToolCallRequested {
                call: RealtimeProviderToolCallRef::new(call_id),
                name: tool_name,
                arguments,
            },
            RealtimeSessionEvent::AssistantTranscriptTruncated {
                response_id,
                item_id,
                audio_played_ms,
                truncated_text,
            } => Self::ProviderAssistantOutputTruncated {
                response: response_id.map(RealtimeProviderResponseRef::new),
                item: RealtimeProviderItemRef::new(item_id),
                audio_played_ms,
                truncated_text,
            },
            RealtimeSessionEvent::RealtimeTranscript { event } => {
                Self::ProviderTranscriptEvent { event }
            }
        }
    }
}

/// An observation plus host-side generation/watermark evidence. Host code can
/// reject stale observations before they reach Meerkat APIs.
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeAdapterObservationEnvelope {
    pub generation: RealtimeAdapterGeneration,
    pub watermark: RealtimeProjectionWatermark,
    pub observation: RealtimeAdapterObservation,
}

impl RealtimeAdapterObservationEnvelope {
    #[must_use]
    pub const fn is_current_for(&self, generation: RealtimeAdapterGeneration) -> bool {
        self.generation.0 == generation.0
    }
}

/// Transport kind selected by a host bootstrap. The tagged enum avoids making
/// a raw websocket URL the public shape of live sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RealtimeTransportKind {
    WebSocket,
    WebRtc,
}

/// Capability projection for one transport bootstrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeTransportCapabilities {
    pub audio_input: bool,
    pub audio_output: bool,
    pub barge_in: bool,
    pub transcript: bool,
    pub provider_native_resume: bool,
}

/// Internal tagged bootstrap for live adapter transports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeTransportBootstrap {
    pub transport: RealtimeTransportKind,
    pub token: String,
    pub url: Option<String>,
    pub input_audio_format: RealtimeAudioFormat,
    pub output_audio_format: RealtimeAudioFormat,
    pub capabilities: RealtimeTransportCapabilities,
}

/// Provider realtime adapter trait. Runtime-owned hosts drive adapters through
/// commands and gate observations back into canonical Meerkat APIs.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealtimeAdapter: Send {
    fn capabilities(&self) -> &RealtimeCapabilities;
    fn status(&self) -> RealtimeAdapterStatus;
    async fn handle_command(&mut self, command: RealtimeAdapterCommand) -> Result<(), LlmError>;
    async fn next_observation(&mut self) -> Result<Option<RealtimeAdapterObservation>, LlmError>;
}

#[cfg(test)]
mod tests {
    use meerkat_contracts::{RealtimeAudioChunk, RealtimeInputChunk, RealtimeTurningMode};
    use meerkat_core::types::Usage;
    use meerkat_core::{
        Provider, RealtimeTranscriptEvent, RealtimeTranscriptRole, SessionLlmIdentity, StopReason,
        ToolResult,
        types::{Message, UserMessage},
    };
    use serde_json::json;

    use crate::realtime_session::RealtimeSessionEvent;

    use super::{
        RealtimeAdapterCommand, RealtimeAdapterContinuity, RealtimeAdapterObservation,
        RealtimeAdapterStatus, RealtimeProjectionSnapshot, RealtimeProjectionWatermark,
        RealtimeProviderItemRef, RealtimeProviderResponseRef, RealtimeProviderToolCallRef,
        RealtimeTransportBootstrap, RealtimeTransportCapabilities, RealtimeTransportKind,
    };

    fn llm_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "gpt-realtime-1.5".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    #[test]
    fn projection_snapshot_converts_to_existing_open_config_without_losing_watermark() {
        let snapshot = RealtimeProjectionSnapshot {
            identity: llm_identity(),
            turning_mode: RealtimeTurningMode::ExplicitCommit,
            visible_tools: Vec::new(),
            seed_messages: vec![Message::User(UserMessage::text("hello"))],
            runtime_system_context: Vec::new(),
            watermark: RealtimeProjectionWatermark {
                version: 1,
                frontier: 42,
                digest: "history-digest".to_string(),
            },
            continuity: RealtimeAdapterContinuity::TranscriptOnly,
            response_nudge_timeout_ms: Some(250),
            response_nudge_max_attempts: Some(2),
        };

        let open_config = snapshot.to_open_config();

        assert_eq!(open_config.llm_identity.model, "gpt-realtime-1.5");
        assert_eq!(
            open_config.turning_mode,
            RealtimeTurningMode::ExplicitCommit
        );
        assert_eq!(open_config.seed_messages.len(), 1);
        assert_eq!(snapshot.watermark.frontier, 42);
        assert_eq!(snapshot.watermark.digest, "history-digest");
        assert_eq!(
            snapshot.continuity,
            RealtimeAdapterContinuity::TranscriptOnly
        );
        assert_eq!(open_config.response_nudge_timeout_ms, Some(250));
        assert_eq!(open_config.response_nudge_max_attempts, Some(2));
    }

    #[test]
    fn commands_model_provider_mechanics_without_runtime_authority() {
        let snapshot = RealtimeProjectionSnapshot {
            identity: llm_identity(),
            turning_mode: RealtimeTurningMode::ProviderManaged,
            visible_tools: Vec::new(),
            seed_messages: Vec::new(),
            runtime_system_context: Vec::new(),
            watermark: RealtimeProjectionWatermark {
                version: 1,
                frontier: 7,
                digest: "seed".to_string(),
            },
            continuity: RealtimeAdapterContinuity::ProviderNative,
            response_nudge_timeout_ms: None,
            response_nudge_max_attempts: None,
        };

        let commands = [
            RealtimeAdapterCommand::OpenSession { snapshot },
            RealtimeAdapterCommand::SendAdmittedInput {
                chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                    mime_type: "audio/pcm".to_string(),
                    data: "AAEC".to_string(),
                    sample_rate_hz: 24_000,
                    channels: 1,
                }),
            },
            RealtimeAdapterCommand::CommitInput,
            RealtimeAdapterCommand::Interrupt {
                reason: "barge-in admitted by runtime".to_string(),
            },
            RealtimeAdapterCommand::TruncateAssistantOutput {
                item: RealtimeProviderItemRef::new("item_1"),
                content_index: 0,
                audio_played_ms: 320,
            },
            RealtimeAdapterCommand::SubmitToolResult {
                result: ToolResult::new("call_1".to_string(), "ok".to_string(), false),
            },
            RealtimeAdapterCommand::SubmitToolError {
                call: RealtimeProviderToolCallRef::new("call_2"),
                error: "denied".to_string(),
            },
        ];

        assert_eq!(commands.len(), 7);
    }

    #[test]
    fn realtime_session_events_map_to_typed_adapter_observations() {
        let tool = RealtimeSessionEvent::ToolCallRequested {
            call_id: "call_1".to_string(),
            tool_name: "lookup".to_string(),
            arguments: json!({ "q": "meerkat" }),
        };
        assert_eq!(
            RealtimeAdapterObservation::from(tool),
            RealtimeAdapterObservation::ProviderToolCallRequested {
                call: RealtimeProviderToolCallRef::new("call_1"),
                name: "lookup".to_string(),
                arguments: json!({ "q": "meerkat" }),
            }
        );

        let transcript = RealtimeSessionEvent::RealtimeTranscript {
            event: RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_1".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "hello".to_string(),
            },
        };
        assert_eq!(
            RealtimeAdapterObservation::from(transcript),
            RealtimeAdapterObservation::ProviderTranscriptEvent {
                event: RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: "item_1".to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    text: "hello".to_string(),
                }
            }
        );

        let final_transcript = RealtimeSessionEvent::InputTranscriptFinalForItem {
            item_id: "item_2".to_string(),
            previous_item_id: Some("item_1".to_string()),
            content_index: 0,
            text: "next".to_string(),
        };
        assert_eq!(
            RealtimeAdapterObservation::from(final_transcript),
            RealtimeAdapterObservation::ProviderTranscriptFinal {
                item: RealtimeProviderItemRef::new("item_2"),
                previous_item: Some(RealtimeProviderItemRef::new("item_1")),
                content_index: 0,
                text: "next".to_string(),
            }
        );
    }

    #[test]
    fn adapter_status_and_bootstrap_keep_transport_truth_tagged() {
        let status = RealtimeAdapterStatus::Degraded {
            reason: "provider_session_expired".to_string(),
        };
        assert!(!status.is_ready());
        assert!(status.is_open());

        let bootstrap = RealtimeTransportBootstrap {
            transport: RealtimeTransportKind::WebSocket,
            token: "opaque-open-token".to_string(),
            url: Some("wss://localhost/live".to_string()),
            input_audio_format: meerkat_contracts::RealtimeAudioFormat::pcm(24_000, 1),
            output_audio_format: meerkat_contracts::RealtimeAudioFormat::pcm(24_000, 1),
            capabilities: RealtimeTransportCapabilities {
                audio_input: true,
                audio_output: true,
                barge_in: true,
                transcript: true,
                provider_native_resume: false,
            },
        };

        assert_eq!(bootstrap.transport, RealtimeTransportKind::WebSocket);
        assert!(!bootstrap.capabilities.provider_native_resume);
    }

    #[test]
    fn turn_completion_observation_uses_wrapped_provider_response_ref() {
        let observation = RealtimeAdapterObservation::from(RealtimeSessionEvent::TurnCompleted {
            response_id: "resp_1".to_string(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        });

        assert_eq!(
            observation,
            RealtimeAdapterObservation::ProviderAssistantTurnCompleted {
                response: RealtimeProviderResponseRef::new("resp_1"),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            }
        );
    }

    #[test]
    fn transcript_roles_remain_available_for_salvaged_transcript_events() {
        let event = RealtimeTranscriptEvent::ItemObserved {
            item_id: "item_a".to_string(),
            previous_item_id: None,
            role: RealtimeTranscriptRole::Assistant,
            response_id: Some("resp_a".to_string()),
        };

        assert!(matches!(
            RealtimeAdapterObservation::ProviderTranscriptEvent { event },
            RealtimeAdapterObservation::ProviderTranscriptEvent { .. }
        ));
    }
}
