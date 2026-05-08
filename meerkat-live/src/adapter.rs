//! Provider session adapter — bridges `LiveAdapter` to `RealtimeSession`.
//!
//! Translates between the Meerkat-owned `LiveAdapter` seam vocabulary and
//! the provider-specific `RealtimeSession`/`RealtimeSessionFactory` layer.
//! Works with any provider's `RealtimeSession` implementation.

use async_trait::async_trait;

use meerkat_contracts::{RealtimeAudioChunk, RealtimeInputChunk, RealtimeTextChunk};
use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode,
    LiveAdapterObservation, LiveAdapterStatus, LiveInputChunk,
};
use meerkat_core::types::ToolResult;
use meerkat_llm_core::LlmError;
use meerkat_llm_core::realtime_session::{RealtimeSession, RealtimeSessionEvent};

/// Adapter bridging `LiveAdapter` to any `RealtimeSession` implementation.
pub struct ProviderSessionAdapter {
    session: Option<Box<dyn RealtimeSession>>,
    status: LiveAdapterStatus,
    emitted_ready: bool,
}

impl ProviderSessionAdapter {
    pub fn new(session: Box<dyn RealtimeSession>) -> Self {
        Self {
            session: Some(session),
            status: LiveAdapterStatus::Ready,
            emitted_ready: false,
        }
    }

    fn session_mut(&mut self) -> Result<&mut Box<dyn RealtimeSession>, LiveAdapterError> {
        self.session.as_mut().ok_or(LiveAdapterError::Closed)
    }

    fn map_llm_error(err: LlmError) -> LiveAdapterError {
        LiveAdapterError::ProviderError {
            code: LiveAdapterErrorCode::ProviderError,
            message: err.to_string(),
        }
    }
}

#[async_trait]
impl LiveAdapter for ProviderSessionAdapter {
    async fn send_command(&mut self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        if !self.status.accepts_commands() {
            return Err(LiveAdapterError::NotReady {
                status: self.status.clone(),
            });
        }
        match command {
            LiveAdapterCommand::Open { .. } => Ok(()),
            LiveAdapterCommand::SendInput { chunk } => {
                let session = self.session_mut()?;
                let input = match chunk {
                    LiveInputChunk::Audio {
                        data,
                        sample_rate_hz,
                        channels,
                    } => {
                        use base64::Engine;
                        RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                            mime_type: "audio/pcm".to_string(),
                            data: base64::engine::general_purpose::STANDARD.encode(&data),
                            sample_rate_hz,
                            channels: channels as u8,
                        })
                    }
                    LiveInputChunk::Text { text } => {
                        RealtimeInputChunk::TextChunk(RealtimeTextChunk { text })
                    }
                    _ => return Ok(()),
                };
                session.send_input(input).await.map_err(Self::map_llm_error)
            }
            LiveAdapterCommand::CommitInput => self
                .session_mut()?
                .commit_turn()
                .await
                .map_err(Self::map_llm_error),
            LiveAdapterCommand::Interrupt => self
                .session_mut()?
                .interrupt()
                .await
                .map_err(Self::map_llm_error),
            LiveAdapterCommand::TruncateAssistantOutput {
                item_id,
                content_index,
                audio_played_ms,
            } => self
                .session_mut()?
                .truncate_assistant_output(item_id, content_index, audio_played_ms)
                .await
                .map_err(Self::map_llm_error),
            LiveAdapterCommand::SubmitToolResult { result } => {
                let tool_result = ToolResult {
                    tool_use_id: result.call_id,
                    content: vec![meerkat_core::types::ContentBlock::Text {
                        text: result.content.to_string(),
                    }],
                    is_error: result.is_error,
                };
                self.session_mut()?
                    .submit_tool_result(tool_result)
                    .await
                    .map_err(Self::map_llm_error)
            }
            LiveAdapterCommand::SubmitToolError { call_id, error } => self
                .session_mut()?
                .submit_tool_error(call_id, error)
                .await
                .map_err(Self::map_llm_error),
            LiveAdapterCommand::Close => {
                if let Some(mut session) = self.session.take() {
                    let _ = session.close().await;
                }
                self.status = LiveAdapterStatus::Closed;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn next_observation(
        &mut self,
    ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        if !self.emitted_ready {
            self.emitted_ready = true;
            return Ok(Some(LiveAdapterObservation::Ready));
        }
        let session = match self.session.as_mut() {
            Some(s) => s,
            None => return Ok(None),
        };
        let event = session.next_event().await.map_err(Self::map_llm_error)?;
        let Some(event) = event else {
            self.status = LiveAdapterStatus::Closed;
            return Ok(Some(LiveAdapterObservation::StatusChanged {
                status: LiveAdapterStatus::Closed,
            }));
        };
        Ok(Some(translate_event(event)))
    }

    fn status(&self) -> &LiveAdapterStatus {
        &self.status
    }

    async fn close(&mut self) -> Result<(), LiveAdapterError> {
        if let Some(mut session) = self.session.take() {
            let _ = session.close().await;
        }
        self.status = LiveAdapterStatus::Closed;
        Ok(())
    }
}

fn translate_event(event: RealtimeSessionEvent) -> LiveAdapterObservation {
    match event {
        RealtimeSessionEvent::InputTranscriptFinal { text } => {
            LiveAdapterObservation::UserTranscriptFinal {
                provider_item_id: String::new(),
                text,
            }
        }
        RealtimeSessionEvent::InputTranscriptFinalForItem { item_id, text, .. } => {
            LiveAdapterObservation::UserTranscriptFinal {
                provider_item_id: item_id,
                text,
            }
        }
        RealtimeSessionEvent::OutputTextDelta { delta } => {
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id: String::new(),
                delta,
            }
        }
        RealtimeSessionEvent::OutputTextDeltaForItem { item_id, delta, .. } => {
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id: item_id,
                delta,
            }
        }
        RealtimeSessionEvent::OutputAudioChunk { chunk } => {
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data)
                .unwrap_or_default();
            LiveAdapterObservation::AssistantAudioChunk {
                data,
                sample_rate_hz: chunk.sample_rate_hz,
                channels: u16::from(chunk.channels),
            }
        }
        RealtimeSessionEvent::TurnCompleted {
            stop_reason, usage, ..
        } => LiveAdapterObservation::TurnCompleted { stop_reason, usage },
        RealtimeSessionEvent::Interrupted { .. } => LiveAdapterObservation::TurnInterrupted,
        RealtimeSessionEvent::ToolCallRequested {
            call_id,
            tool_name,
            arguments,
        } => LiveAdapterObservation::ToolCallRequested {
            provider_call_id: call_id,
            tool_name,
            arguments,
        },
        RealtimeSessionEvent::AssistantTranscriptTruncated {
            item_id,
            truncated_text,
            ..
        } => LiveAdapterObservation::AssistantTranscriptTruncated {
            provider_item_id: item_id,
            text: truncated_text.unwrap_or_default(),
        },
        RealtimeSessionEvent::InputTranscriptPartial { .. }
        | RealtimeSessionEvent::TurnStarted
        | RealtimeSessionEvent::TurnCommitted
        | RealtimeSessionEvent::OutputVideoChunk { .. }
        | RealtimeSessionEvent::RealtimeTranscript { .. } => {
            LiveAdapterObservation::StatusChanged {
                status: LiveAdapterStatus::Ready,
            }
        }
    }
}
