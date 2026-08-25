#![warn(clippy::all, clippy::pedantic, clippy::nursery, clippy::cargo)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::multiple_crate_versions)]

pub mod error;
pub mod protocol;
pub mod sdk;
pub mod transport;

pub use error::{ApiErrorType, Error, Result, ServerError};
pub use protocol::client_events::ClientEvent;
pub use protocol::models::{
    ApprovalFilter, ApprovalMode, AudioConfig, AudioFormat, CachedTokenDetails, ContentPart,
    ConversationMode, DEFAULT_MODEL, Eagerness, GPT_REALTIME_2, GPT_REALTIME_TRANSLATE,
    GPT_REALTIME_WHISPER, Infinite, InputAudioConfig, InputAudioTranscription, InputItem,
    InputTokenDetails, Item, ItemStatus, MaxTokens, McpError, McpToolConfig, McpToolInfo, Modality,
    NoiseReduction, NoiseReductionType, OutputAudioConfig, OutputModalities, OutputTokenDetails,
    PromptRef, ReasoningConfig, ReasoningEffort, RequireApproval, Response, ResponseConfig,
    ResponsePhase, ResponseStatus, RetentionRatioTruncation, Role, Session, SessionConfig,
    SessionKind, SessionUpdate, SessionUpdateConfig, Temperature, TokenLimits, Tool, ToolChoice,
    ToolChoiceMode, Tracing, TracingAuto, TracingConfig, TranscriptionSessionUpdateConfig,
    Truncation, TruncationStrategy, TruncationType, Usage, Voice,
};
pub use protocol::server_events::ServerEvent;
pub use sdk::{
    AudioChunk, AudioIn, EventStream, Realtime, RealtimeBuilder, ResponseBuilder, SdkEvent,
    Session as RealtimeSession, SessionHandle, ToolCall, ToolFuture, ToolRegistry, ToolResult,
    ToolSpec, TranscriptChunk, VoiceEvent, VoiceEventStream, VoiceSessionBuilder,
};

use crate::protocol::models;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use serde_json::from_str;
use std::future::Future;
use tokio_tungstenite::tungstenite::protocol::Message;
use transport::ws::WsStream;

const TRACE_LOG_MAX_BYTES: usize = 1024;
const MAX_INPUT_AUDIO_CHUNK_BYTES: usize = 15 * 1024 * 1024;
const TRACE_TRUNCATE_SUFFIX: &str = "... (truncated)";

/// The main client for interacting with the `OpenAI` Realtime API.
///
/// Thread safety: `RealtimeClient` is `Send` but not `Sync` because the underlying
/// WebSocket stream is not `Sync`.
#[must_use]
pub struct RealtimeClient {
    stream: WsStream,
}

impl RealtimeClient {
    /// Connect to the `OpenAI` Realtime API.
    ///
    /// # Errors
    /// Returns an error if the connection fails or if the URL is invalid.
    pub async fn connect(
        api_key: &str,
        model: Option<&str>,
        call_id: Option<&str>,
    ) -> Result<Self> {
        let stream = transport::ws::connect(api_key, model, call_id).await?;
        Ok(Self { stream })
    }

    /// Connect to the dedicated Realtime translation WebSocket endpoint.
    ///
    /// # Errors
    /// Returns an error if the connection fails or if the URL is invalid.
    pub async fn connect_translation(
        api_key: &str,
        model: Option<&str>,
        safety_identifier: Option<&str>,
    ) -> Result<Self> {
        let stream = transport::ws::connect_with_options(
            api_key,
            transport::ws::WsConnectOptions {
                model,
                safety_identifier,
                target: transport::ws::WsConnectTarget::Translation,
                ..transport::ws::WsConnectOptions::default()
            },
        )
        .await?;
        Ok(Self { stream })
    }

    /// Connect to the Realtime transcription WebSocket endpoint.
    ///
    /// # Errors
    /// Returns an error if the connection fails or if the URL is invalid.
    pub async fn connect_transcription(
        api_key: &str,
        safety_identifier: Option<&str>,
    ) -> Result<Self> {
        let stream = transport::ws::connect_with_options(
            api_key,
            transport::ws::WsConnectOptions {
                safety_identifier,
                target: transport::ws::WsConnectTarget::Transcription,
                ..transport::ws::WsConnectOptions::default()
            },
        )
        .await?;
        Ok(Self { stream })
    }

    /// Connect to the Realtime API with explicit WebSocket options.
    ///
    /// # Errors
    /// Returns an error if the connection fails or if the URL is invalid.
    pub async fn connect_with_options(
        api_key: &str,
        options: transport::ws::WsConnectOptions<'_>,
    ) -> Result<Self> {
        let stream = transport::ws::connect_with_options(api_key, options).await?;
        Ok(Self { stream })
    }

    /// Send a client event to the server.
    ///
    /// # Errors
    /// Returns an error if serialization fails or if the WebSocket send fails.
    pub async fn send(&mut self, event: ClientEvent) -> Result<()> {
        validate_client_event(&event)?;
        let json = serde_json::to_string(&event)?;
        tracing::trace!(
            "Sending event: {}",
            safe_truncate(&json, TRACE_LOG_MAX_BYTES)
        );
        self.stream.send(Message::Text(json.into())).await?;
        Ok(())
    }

    /// Receive the next server event.
    ///
    /// # Errors
    /// Returns an error if deserialization fails or if the WebSocket fails.
    pub async fn next_event(&mut self) -> Result<Option<ServerEvent>> {
        while let Some(msg) = self.stream.next().await {
            match msg? {
                Message::Text(text) => {
                    tracing::trace!(
                        "Received event: {}",
                        safe_truncate(&text, TRACE_LOG_MAX_BYTES)
                    );
                    return Ok(Some(from_str::<ServerEvent>(&text)?));
                }
                Message::Close(_) => {
                    tracing::info!("WebSocket connection closed by server");
                    return Ok(None);
                }
                Message::Ping(payload) => {
                    tracing::debug!("Received Ping, sending Pong");
                    self.stream.send(Message::Pong(payload)).await?;
                }
                _ => (),
            }
        }
        Ok(None)
    }

    /// Split the client into a sender and a receiver for concurrent usage.
    pub fn split(self) -> (RealtimeSender, RealtimeReceiver) {
        let (write, read) = self.stream.split();
        (RealtimeSender { write }, RealtimeReceiver { read })
    }

    /// Re-unify a split client.
    ///
    /// # Errors
    /// Returns an error if the split halves don't match or cannot be reunited.
    #[allow(clippy::result_large_err)]
    pub fn unsplit(sender: RealtimeSender, receiver: RealtimeReceiver) -> Result<Self> {
        let stream = receiver.read.reunite(sender.write)?;
        Ok(Self { stream })
    }
}

fn safe_truncate(s: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if s.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(s);
    }

    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    std::borrow::Cow::Owned(format!(
        "{} {} {} bytes",
        &s[..end],
        TRACE_TRUNCATE_SUFFIX,
        s.len() - end
    ))
}

/// The sending half of a split `RealtimeClient`.
pub struct RealtimeSender {
    write: futures::stream::SplitSink<WsStream, Message>,
}

impl RealtimeSender {
    /// Send a client event.
    ///
    /// # Errors
    /// Returns an error if serialization or sending fails.
    pub async fn send(&mut self, event: ClientEvent) -> Result<()> {
        validate_client_event(&event)?;
        let json = serde_json::to_string(&event)?;
        tracing::trace!(
            "Sending event (split): {}",
            safe_truncate(&json, TRACE_LOG_MAX_BYTES)
        );
        self.write.send(Message::Text(json.into())).await?;
        Ok(())
    }
}

#[allow(clippy::result_large_err)]
fn validate_client_event(event: &ClientEvent) -> Result<()> {
    match event {
        ClientEvent::InputAudioBufferAppend { audio, .. }
        | ClientEvent::SessionInputAudioBufferAppend { audio, .. } => {
            let size = estimate_base64_decoded_len(audio)?;
            if size > MAX_INPUT_AUDIO_CHUNK_BYTES {
                return Err(Error::InvalidClientEvent(format!(
                    "input_audio_buffer.append exceeds 15MB ({size} bytes)",
                )));
            }
        }
        ClientEvent::SessionUpdate { session, .. } => {
            validate_session_update(session.as_ref())?;
        }
        ClientEvent::TranscriptionSessionUpdate { session, .. } => {
            validate_transcription_session_update(session.as_ref())?;
        }
        ClientEvent::ResponseCreate {
            response: Some(config),
            ..
        } => {
            validate_response_config(config.as_ref())?;
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_transcription_session_update(
    config: &models::TranscriptionSessionUpdateConfig,
) -> Result<()> {
    if let Some(format) = &config.input_audio_format {
        validate_audio_format(format)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_session_update(session: &models::SessionUpdate) -> Result<()> {
    let config = &session.config;
    if let Some(format) = &config.input_audio_format {
        validate_audio_format(format)?;
    }
    if let Some(format) = &config.output_audio_format {
        validate_audio_format(format)?;
    }
    if let Some(audio) = &config.audio {
        validate_audio_config(audio)?;
    }
    if let Some(tools) = &config.tools {
        validate_tools(tools)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_response_config(config: &models::ResponseConfig) -> Result<()> {
    if let Some(audio) = &config.audio {
        validate_audio_config(audio)?;
    }
    if let Some(format) = &config.input_audio_format {
        validate_audio_format(format)?;
        if let Some(audio) = &config.audio {
            if let Some(input) = &audio.input {
                if let Some(nested) = &input.format {
                    if nested != format {
                        return Err(Error::InvalidClientEvent(
                            "response.input_audio_format conflicts with response.audio.input.format"
                                .to_string(),
                        ));
                    }
                }
            }
        }
    }
    if let Some(tools) = &config.tools {
        validate_tools(tools)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_audio_config(audio: &models::AudioConfig) -> Result<()> {
    if let Some(input) = &audio.input {
        validate_input_audio_config(input)?;
    }
    if let Some(output) = &audio.output {
        validate_output_audio_config(output)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_input_audio_config(audio: &models::InputAudioConfig) -> Result<()> {
    if let Some(format) = &audio.format {
        validate_audio_format(format)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_output_audio_config(audio: &models::OutputAudioConfig) -> Result<()> {
    if let Some(format) = &audio.format {
        validate_audio_format(format)?;
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_audio_format(format: &models::AudioFormat) -> Result<()> {
    format.validate()?;
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_tools(tools: &[models::Tool]) -> Result<()> {
    for tool in tools {
        if let models::Tool::Mcp(config) = tool {
            config.validate()?;
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn estimate_base64_decoded_len(s: &str) -> Result<usize> {
    let bytes = s.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err(Error::InvalidClientEvent(
            "input_audio_buffer.append invalid base64 length".to_string(),
        ));
    }

    let mut padding = 0;
    let mut seen_padding = false;
    for &b in bytes {
        if b == b'=' {
            seen_padding = true;
            padding += 1;
            continue;
        }
        if seen_padding {
            return Err(Error::InvalidClientEvent(
                "input_audio_buffer.append invalid base64 padding".to_string(),
            ));
        }
        let is_valid = matches!(b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/'
        );
        if !is_valid {
            return Err(Error::InvalidClientEvent(
                "input_audio_buffer.append invalid base64 character".to_string(),
            ));
        }
    }

    if padding > 2 {
        return Err(Error::InvalidClientEvent(
            "input_audio_buffer.append invalid base64 padding length".to_string(),
        ));
    }

    Ok(bytes.len() / 4 * 3 - padding)
}

/// The receiving half of a split `RealtimeClient`.
pub struct RealtimeReceiver {
    read: futures::stream::SplitStream<WsStream>,
}

impl RealtimeReceiver {
    /// Exposes an asynchronous stream of `Result<ServerEvent>` that preserves Errors.
    #[must_use]
    #[allow(clippy::result_large_err)]
    pub fn try_into_stream(self) -> BoxStream<'static, Result<ServerEvent>> {
        self.read
            .map(|res| res.map_err(Error::from))
            .filter_map(|res| async move {
                match res {
                    Ok(Message::Text(text)) => {
                        tracing::trace!(
                            "Received event (stream): {}",
                            safe_truncate(&text, TRACE_LOG_MAX_BYTES)
                        );
                        Some(from_str::<ServerEvent>(&text).map_err(Error::from))
                    }
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .boxed()
    }
}

/// Forward raw server events from a stream to a handler until EOF.
///
/// # Errors
/// Returns the first stream error or handler error.
pub async fn pump_raw_event_stream<S, F, Fut>(mut stream: S, mut handler: F) -> Result<()>
where
    S: futures::Stream<Item = Result<ServerEvent>> + Unpin,
    F: FnMut(ServerEvent) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    while let Some(event) = stream.next().await {
        handler(event?).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::ready;

    #[tokio::test]
    async fn raw_event_pump_forwards_events_until_eof() {
        let stream = futures::stream::iter(vec![
            Ok(ServerEvent::InputAudioBufferCleared {
                event_id: "evt_1".to_string(),
            }),
            Ok(ServerEvent::InputAudioBufferCleared {
                event_id: "evt_2".to_string(),
            }),
        ]);
        let mut event_ids = Vec::new();

        pump_raw_event_stream(stream, |event| {
            if let ServerEvent::InputAudioBufferCleared { event_id } = event {
                event_ids.push(event_id);
            }
            ready(Ok(()))
        })
        .await
        .expect("pump succeeds");

        assert_eq!(event_ids, ["evt_1", "evt_2"]);
    }

    #[tokio::test]
    async fn raw_event_pump_returns_first_stream_error() {
        let stream = futures::stream::iter(vec![Err(Error::ConnectionClosed)]);

        let err = pump_raw_event_stream(stream, |_| ready(Ok(())))
            .await
            .expect_err("stream error should be returned");

        assert!(matches!(err, Error::ConnectionClosed));
    }

    #[tokio::test]
    async fn raw_event_pump_returns_first_handler_error() {
        let stream = futures::stream::iter(vec![Ok(ServerEvent::InputAudioBufferCleared {
            event_id: "evt_1".to_string(),
        })]);

        let err = pump_raw_event_stream(stream, |_| ready(Err(Error::ConnectionClosed)))
            .await
            .expect_err("handler error should be returned");

        assert!(matches!(err, Error::ConnectionClosed));
    }
}
