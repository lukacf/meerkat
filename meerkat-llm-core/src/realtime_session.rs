//! Provider-neutral realtime session traits for product-layer channel clients.
//!
//! These types live in `meerkat-client` because they describe provider transport
//! capabilities and normalized event mapping, not runtime lifecycle truth.

use async_trait::async_trait;
use meerkat_contracts::{
    RealtimeAudioChunk, RealtimeCapabilities, RealtimeEvent, RealtimeInputChunk,
    RealtimeTurningMode, RealtimeVideoChunk,
};
use meerkat_core::{PendingSystemContextAppend, ToolResult};
use meerkat_core::{SessionLlmIdentity, StopReason, ToolDef, types::Message, types::Usage};
use serde_json::Value;

use crate::LlmError;

/// Advanced/internal target for attaching to an existing provider session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeExternalSessionTarget {
    pub provider_session_id: String,
}

impl RealtimeExternalSessionTarget {
    /// Construct a provider session target, rejecting blank identifiers.
    pub fn new(provider_session_id: impl Into<String>) -> Result<Self, LlmError> {
        let provider_session_id = provider_session_id.into();
        if provider_session_id.trim().is_empty() {
            return Err(LlmError::InvalidRequest {
                message: "provider realtime session id must not be empty".to_string(),
            });
        }
        Ok(Self {
            provider_session_id,
        })
    }
}

/// Provider-neutral realtime event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeSessionEvent {
    InputTranscriptPartial {
        text: String,
    },
    InputTranscriptFinal {
        text: String,
    },
    TurnStarted,
    TurnCommitted,
    TurnCompleted {
        stop_reason: StopReason,
        usage: Usage,
    },
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
        arguments: Value,
    },
    /// The assistant output identified by `item_id` was truncated at
    /// `audio_played_ms` because the user barged in. `truncated_text` is the
    /// heard prefix, or `None` if the provider has not yet re-projected it.
    AssistantTranscriptTruncated {
        item_id: String,
        audio_played_ms: u64,
        truncated_text: Option<String>,
    },
}

impl RealtimeSessionEvent {
    /// Project an internal provider event into the public channel event shape.
    #[must_use]
    pub fn to_public_event(&self) -> RealtimeEvent {
        match self {
            Self::InputTranscriptPartial { text } => {
                RealtimeEvent::InputTranscriptPartial { text: text.clone() }
            }
            Self::InputTranscriptFinal { text } => RealtimeEvent::InputTranscriptFinal {
                text: text.clone(),
                // Provider-layer prosody annotations are not surfaced
                // through the internal adapter today; when a provider
                // starts exposing structured prosody, the adapter fills
                // this field before emitting the public event.
                prosody_hint: None,
            },
            Self::TurnStarted => RealtimeEvent::TurnStarted,
            Self::TurnCommitted => RealtimeEvent::TurnCommitted,
            Self::TurnCompleted { .. } => RealtimeEvent::TurnCompleted,
            Self::OutputTextDelta { delta } => RealtimeEvent::OutputTextDelta {
                delta: delta.clone(),
            },
            Self::OutputAudioChunk { chunk } => RealtimeEvent::OutputAudioChunk {
                chunk: chunk.clone(),
            },
            Self::OutputVideoChunk { chunk } => RealtimeEvent::OutputVideoChunk {
                chunk: chunk.clone(),
            },
            Self::Interrupted => RealtimeEvent::Interrupted,
            Self::ToolCallRequested {
                call_id, tool_name, ..
            } => RealtimeEvent::ToolCallRequested {
                call_id: call_id.clone(),
                tool_name: tool_name.clone(),
            },
            Self::AssistantTranscriptTruncated {
                item_id,
                audio_played_ms,
                truncated_text,
            } => RealtimeEvent::AssistantTranscriptTruncated {
                item_id: item_id.clone(),
                audio_played_ms: *audio_played_ms,
                truncated_text: truncated_text.clone(),
            },
        }
    }
}

/// Provider-neutral realtime session surface.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealtimeSession: Send {
    /// Report the product-facing capability set the provider can honor.
    fn capabilities(&self) -> &RealtimeCapabilities;

    /// Report the turning mode selected when the session was opened.
    fn turning_mode(&self) -> RealtimeTurningMode;

    /// Refresh the provider's projection of canonical Meerkat session state.
    ///
    /// This is projection-only: canonical Meerkat history, visible tools, and
    /// related policy remain the semantic owner. Providers update their local
    /// session view from the latest canonical open config before the next user
    /// turn, rather than becoming a second owner of conversation truth.
    async fn refresh_projection(
        &mut self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<(), LlmError>;

    /// Stream one modality-neutral input chunk into the provider session.
    async fn send_input(&mut self, chunk: RealtimeInputChunk) -> Result<(), LlmError>;

    /// Commit the staged turn when the session is using explicit commit.
    async fn commit_turn(&mut self) -> Result<(), LlmError>;

    /// Interrupt the currently active provider response, if any.
    async fn interrupt(&mut self) -> Result<(), LlmError>;

    /// Truncate the assistant output for `item_id` to `audio_played_ms` so the
    /// canonical session transcript reflects what the user actually heard
    /// before barging in. The adapter is expected to eventually emit
    /// [`RealtimeSessionEvent::AssistantTranscriptTruncated`] with the
    /// re-projected prefix (or a best-effort approximation if the provider
    /// cannot supply exact text).
    async fn truncate_assistant_output(
        &mut self,
        item_id: String,
        content_index: u32,
        audio_played_ms: u64,
    ) -> Result<(), LlmError>;

    /// Submit a completed tool result back into the provider session so its
    /// response can continue.
    async fn submit_tool_result(&mut self, result: ToolResult) -> Result<(), LlmError>;

    /// Submit a tool-dispatch error back into the provider session.
    async fn submit_tool_error(&mut self, call_id: String, error: String) -> Result<(), LlmError>;

    /// Read the next normalized realtime session event.
    async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError>;

    /// Close the provider session and release any local transport state.
    async fn close(&mut self) -> Result<(), LlmError>;
}

/// Canonical live session projection used to open a provider-backed realtime session.
///
/// This is the product-session equivalent of a build seam: the provider session
/// must be opened from the currently-owned Meerkat session identity, visible
/// tools, and committed transcript instead of inventing a parallel provider-only
/// conversation.
#[derive(Debug, Clone)]
pub struct RealtimeSessionOpenConfig {
    pub turning_mode: RealtimeTurningMode,
    pub llm_identity: SessionLlmIdentity,
    pub visible_tools: Vec<ToolDef>,
    pub seed_messages: Vec<Message>,
    /// Runtime-authored system context carried as typed provenance.
    ///
    /// Provider adapters must treat this as the only authoritative realtime
    /// reconstruction source for runtime context. Rendered transcript markers
    /// are projections only and must not be parsed back into authority.
    pub runtime_system_context: Vec<PendingSystemContextAppend>,
    /// Per-channel override for the "nudge the provider" timeout the OpenAI
    /// adapter uses while waiting for the first real delta after a turn is
    /// admitted. `None` inherits the adapter's compile-time default.
    pub response_nudge_timeout_ms: Option<u64>,
    /// Per-channel override for the maximum number of nudge attempts before
    /// the adapter gives up. `None` inherits the adapter default.
    pub response_nudge_max_attempts: Option<u8>,
}

impl RealtimeSessionOpenConfig {
    #[must_use]
    pub fn new(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
    ) -> Self {
        Self {
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            runtime_system_context: Vec::new(),
            response_nudge_timeout_ms: None,
            response_nudge_max_attempts: None,
        }
    }

    /// Builder-style typed runtime context for provider reconstruction.
    #[must_use]
    pub fn with_runtime_system_context(
        mut self,
        runtime_system_context: Vec<PendingSystemContextAppend>,
    ) -> Self {
        self.runtime_system_context = runtime_system_context;
        self
    }

    /// Builder-style override for the per-channel nudge timeout.
    #[must_use]
    pub fn with_response_nudge_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.response_nudge_timeout_ms = timeout_ms;
        self
    }

    /// Builder-style override for the per-channel nudge max attempts.
    #[must_use]
    pub fn with_response_nudge_max_attempts(mut self, max_attempts: Option<u8>) -> Self {
        self.response_nudge_max_attempts = max_attempts;
        self
    }
}

/// Factory for provider-neutral realtime sessions.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealtimeSessionFactory: Send + Sync {
    /// Report the provider/product capability set exposed by this factory.
    fn capabilities(&self) -> RealtimeCapabilities;

    /// Open a provider-created realtime session using the selected turning mode.
    async fn open_session(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError>;

    /// Attach to an existing provider-managed realtime session.
    async fn attach_external_session(
        &self,
        target: &RealtimeExternalSessionTarget,
        turning_mode: RealtimeTurningMode,
    ) -> Result<Box<dyn RealtimeSession>, LlmError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn external_session_target_rejects_blank_provider_id() {
        let error = match RealtimeExternalSessionTarget::new("   ") {
            Ok(_) => panic!("blank provider id must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, LlmError::InvalidRequest { .. }));
    }

    #[test]
    fn tool_call_projection_strips_internal_arguments_for_public_event() {
        let public = RealtimeSessionEvent::ToolCallRequested {
            call_id: "call_1".to_string(),
            tool_name: "lookup".to_string(),
            arguments: serde_json::json!({ "q": "otter" }),
        }
        .to_public_event();

        assert_eq!(
            public,
            RealtimeEvent::ToolCallRequested {
                call_id: "call_1".to_string(),
                tool_name: "lookup".to_string(),
            }
        );
    }
}
