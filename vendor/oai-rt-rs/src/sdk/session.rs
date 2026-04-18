use crate::protocol::client_events::ClientEvent;
use crate::protocol::models::{
    ContentPart, Item, ItemStatus, ResponseConfig, SessionConfig, SessionUpdate,
    SessionUpdateConfig,
};
use crate::protocol::server_events::ServerEvent;
use crate::{Error, Result};

use super::events::{EventStream, SdkEvent};
use super::handlers::EventHandlers;
use super::response::ResponseBuilder;
use super::tools::{ToolCall, ToolDispatcher, ToolResult};
use super::transport::Transport;
use super::voice::{VoiceEvent, VoiceEventStream};
use base64::Engine as _;
use base64::engine::general_purpose;
use futures::Stream;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone)]
pub struct SessionHandle {
    sender: mpsc::Sender<Command>,
}

pub struct AudioIn<'a> {
    session: &'a Session,
}

pub struct Session {
    sender: mpsc::Sender<Command>,
    text_rx: mpsc::Receiver<String>,
    event_rx: mpsc::Receiver<SdkEvent>,
    voice_rx: mpsc::Receiver<VoiceEvent>,
    audio_rx: mpsc::Receiver<super::voice::AudioChunk>,
    transcript_rx: mpsc::Receiver<super::voice::TranscriptChunk>,
    active_response_id: Arc<Mutex<Option<String>>>,
}

impl Session {
    #[must_use]
    pub fn handle(&self) -> SessionHandle {
        SessionHandle {
            sender: self.sender.clone(),
        }
    }

    /// Convenience audio input helper.
    #[must_use]
    pub const fn audio(&self) -> AudioIn<'_> {
        AudioIn { session: self }
    }

    /// Send a single user text message and return immediately.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn say(&self, text: &str) -> Result<()> {
        let item = Item::Message {
            id: None,
            status: None,
            role: crate::protocol::models::Role::User,
            content: vec![ContentPart::InputText {
                text: text.to_string(),
            }],
        };

        let event = ClientEvent::ConversationItemCreate {
            event_id: None,
            previous_item_id: None,
            item: Box::new(item),
        };

        self.send_event(event).await
    }

    /// Await the next completed text response, if any.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the stream fails.
    pub async fn next_text(&mut self) -> Result<Option<String>> {
        Ok(self.text_rx.recv().await)
    }

    /// Await the next SDK event.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the stream fails.
    pub async fn next_event(&mut self) -> Result<Option<SdkEvent>> {
        Ok(self.event_rx.recv().await)
    }

    /// Stream SDK events.
    #[must_use]
    pub const fn events(&mut self) -> EventStream<'_> {
        EventStream::new(&mut self.event_rx)
    }

    /// Await the next voice event.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the stream fails.
    pub async fn next_voice_event(&mut self) -> Result<Option<VoiceEvent>> {
        Ok(self.voice_rx.recv().await)
    }

    /// Stream voice events.
    #[must_use]
    pub const fn voice_events(&mut self) -> VoiceEventStream<'_> {
        VoiceEventStream::new(&mut self.voice_rx)
    }

    /// Returns the ID of the currently active response, if any.
    pub async fn active_response_id(&self) -> Option<String> {
        self.active_response_id.lock().await.clone()
    }

    /// Returns true if the session is currently generating a response.
    pub async fn is_responding(&self) -> bool {
        self.active_response_id.lock().await.is_some()
    }

    /// Await the next decoded audio chunk.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the stream fails.
    pub async fn next_audio_chunk(&mut self) -> Result<Option<super::voice::AudioChunk>> {
        Ok(self.audio_rx.recv().await)
    }

    /// Await the next transcript chunk.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the stream fails.
    pub async fn next_transcript(&mut self) -> Result<Option<super::voice::TranscriptChunk>> {
        Ok(self.transcript_rx.recv().await)
    }

    /// Send a raw protocol event.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn send_raw(&self, event: ClientEvent) -> Result<()> {
        self.send_event(event).await
    }

    /// Append PCM16 audio samples to the input audio buffer.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn audio_in_append_pcm16(&self, samples: &[i16]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }

        let mut buf = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            buf.extend_from_slice(&sample.to_le_bytes());
        }
        self.audio_in_append_bytes(&buf).await
    }

    /// Append PCM16 audio samples and commit the buffer in one step.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn send_audio_pcm16(&self, samples: &[i16]) -> Result<()> {
        self.audio_in_append_pcm16(samples).await?;
        self.audio_in_commit().await
    }

    /// Append raw PCM16 bytes to the input audio buffer.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn audio_in_append_bytes(&self, pcm_bytes: &[u8]) -> Result<()> {
        if pcm_bytes.is_empty() {
            return Ok(());
        }
        let encoded = general_purpose::STANDARD.encode(pcm_bytes);
        let event = ClientEvent::InputAudioBufferAppend {
            event_id: None,
            audio: encoded,
        };
        self.send_event(event).await
    }

    /// Append raw PCM16 bytes and commit the buffer in one step.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn send_audio_bytes(&self, pcm_bytes: &[u8]) -> Result<()> {
        self.audio_in_append_bytes(pcm_bytes).await?;
        self.audio_in_commit().await
    }

    /// Stream PCM16 audio chunks into the input buffer, committing after each chunk.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn stream_audio_pcm16<S>(&self, mut stream: S) -> Result<()>
    where
        S: Stream<Item = Vec<i16>> + Unpin,
    {
        while let Some(chunk) = stream.next().await {
            self.send_audio_pcm16(&chunk).await?;
        }
        Ok(())
    }

    /// Stream raw PCM16 byte chunks into the input buffer, committing after each chunk.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn stream_audio_bytes<S>(&self, mut stream: S) -> Result<()>
    where
        S: Stream<Item = Vec<u8>> + Unpin,
    {
        while let Some(chunk) = stream.next().await {
            self.send_audio_bytes(&chunk).await?;
        }
        Ok(())
    }

    /// Commit the current input audio buffer.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn audio_in_commit(&self) -> Result<()> {
        let event = ClientEvent::InputAudioBufferCommit { event_id: None };
        self.send_event(event).await
    }

    /// Clear the input audio buffer.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn audio_in_clear(&self) -> Result<()> {
        let event = ClientEvent::InputAudioBufferClear { event_id: None };
        self.send_event(event).await
    }

    /// Dispatch a tool call to the registry.
    ///
    /// # Errors
    /// Returns an error if the tool is missing or execution fails.
    pub async fn run_tool(&self, call: ToolCall) -> Result<ToolResult> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(Command::RunTool { call, respond: tx })
            .await
            .map_err(|_| Error::ConnectionClosed)?;
        rx.await.map_err(|_| Error::ConnectionClosed)?
    }

    /// Apply a session update.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the update fails.
    pub async fn update_session(&self, update: SessionUpdate) -> Result<()> {
        let event = ClientEvent::SessionUpdate {
            event_id: None,
            session: Box::new(update),
        };
        self.send_event(event).await
    }

    /// Create a response builder.
    #[must_use]
    pub fn response(&self) -> ResponseBuilder {
        ResponseBuilder::new()
    }

    /// Send a response.create event with the provided config.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn send_response(&self, config: ResponseConfig) -> Result<()> {
        let event = ClientEvent::ResponseCreate {
            event_id: None,
            response: Some(Box::new(config)),
        };
        self.send_event(event).await
    }

    /// Request a response using server defaults.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn respond(&self) -> Result<()> {
        let event = ClientEvent::ResponseCreate {
            event_id: None,
            response: None,
        };
        self.send_event(event).await
    }

    /// Clear output audio and cancel any active response (barge-in).
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn barge_in(&self) -> Result<()> {
        self.clear_output_audio().await?;
        let response_id = { self.active_response_id.lock().await.clone() };
        if let Some(id) = response_id {
            let event = ClientEvent::ResponseCancel {
                event_id: None,
                response_id: Some(id),
            };
            self.send_event(event).await?;
        }
        Ok(())
    }

    /// Clear the output audio buffer.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn clear_output_audio(&self) -> Result<()> {
        let event = ClientEvent::OutputAudioBufferClear { event_id: None };
        self.send_event(event).await
    }

    /// Send a user message and await the next completed text response.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the stream fails.
    pub async fn ask(&mut self, text: &str) -> Result<Option<String>> {
        self.say(text).await?;
        self.respond().await?;
        self.next_text().await
    }

    /// Approve an MCP tool request.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn approve_mcp(&self, approval_request_id: &str, reason: Option<&str>) -> Result<()> {
        self.mcp_approval(approval_request_id, true, reason).await
    }

    /// Deny an MCP tool request.
    ///
    /// # Errors
    /// Returns an error if the SDK is not fully initialized or the send fails.
    pub async fn deny_mcp(&self, approval_request_id: &str, reason: Option<&str>) -> Result<()> {
        self.mcp_approval(approval_request_id, false, reason).await
    }

    async fn mcp_approval(
        &self,
        approval_request_id: &str,
        approve: bool,
        reason: Option<&str>,
    ) -> Result<()> {
        let item = Item::McpApprovalResponse {
            id: None,
            status: Some(ItemStatus::Completed),
            approval_request_id: approval_request_id.to_string(),
            approve,
            reason: reason.map(str::to_string),
        };

        let event = ClientEvent::ConversationItemCreate {
            event_id: None,
            previous_item_id: None,
            item: Box::new(item),
        };
        self.send_event(event).await
    }

    async fn send_event(&self, event: ClientEvent) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(Command::SendWithResponse { event, respond: tx })
            .await
            .map_err(|_| Error::ConnectionClosed)?;
        rx.await.map_err(|_| Error::ConnectionClosed)??;
        Ok(())
    }

    pub(crate) fn from_transport(
        mut transport: Box<dyn Transport>,
        handlers: EventHandlers,
        dispatcher: Arc<dyn ToolDispatcher>,
        auto_barge_in: bool,
        auto_tool_response: bool,
    ) -> Self {
        let (sender_tx, mut sender_rx) = mpsc::channel(32);
        let (text_tx, text_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = mpsc::channel(128);
        let (voice_tx, voice_rx) = mpsc::channel(128);
        let (audio_tx, audio_rx) = mpsc::channel(128);
        let (transcript_tx, transcript_rx) = mpsc::channel(128);

        let active_response_id = Arc::new(Mutex::new(None));
        let active_response_id_loop = Arc::clone(&active_response_id);

        tokio::spawn(async move {
            let mut buffers = HashMap::new();
            loop {
                let mut ctx = EventContext {
                    handlers: &handlers,
                    dispatcher: dispatcher.as_ref(),
                    buffers: &mut buffers,
                    event_tx: &event_tx,
                    text_tx: &text_tx,
                    voice_tx: &voice_tx,
                    audio_tx: &audio_tx,
                    transcript_tx: &transcript_tx,
                    active_response_id: &active_response_id_loop,
                    auto_barge_in,
                    auto_tool_response,
                };

                tokio::select! {
                    cmd = sender_rx.recv() => {
                        match cmd {
                            Some(Command::SendWithResponse { event, respond }) => {
                                let _ = respond.send(transport.send(event).await);
                            }
                            Some(Command::RunTool { call, respond }) => {
                                let res = dispatcher.dispatch(call).await;
                                let _ = respond.send(res);
                            }
                            Some(Command::GetActiveResponseId { respond }) => {
                                let _ = respond.send(active_response_id_loop.lock().await.clone());
                            }
                            None => break,
                        }
                    }
                    res = transport.next_event() => {
                        match res {
                            Ok(Some(evt)) => {
                                handle_server_event(evt, &mut ctx, &mut transport).await;
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
            }
        });

        Self {
            sender: sender_tx,
            text_rx,
            event_rx,
            voice_rx,
            audio_rx,
            transcript_rx,
            active_response_id,
        }
    }
}

impl AudioIn<'_> {
    /// Append PCM16 samples to the input buffer.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn push_pcm16(&self, samples: &[i16]) -> Result<()> {
        self.session.audio_in_append_pcm16(samples).await
    }

    /// Append PCM16 bytes to the input buffer.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn push_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.session.audio_in_append_bytes(bytes).await
    }

    /// Commit the current input buffer.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn commit(&self) -> Result<()> {
        self.session.audio_in_commit().await
    }

    /// Clear the input buffer.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn clear(&self) -> Result<()> {
        self.session.audio_in_clear().await
    }

    /// Send PCM16 samples (append + commit).
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn send_pcm16(&self, samples: &[i16]) -> Result<()> {
        self.session.send_audio_pcm16(samples).await
    }

    /// Send PCM16 bytes (append + commit).
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.session.send_audio_bytes(bytes).await
    }
}

struct EventContext<'a> {
    handlers: &'a EventHandlers,
    dispatcher: &'a dyn ToolDispatcher,
    buffers: &'a mut HashMap<(String, u32), String>,
    event_tx: &'a mpsc::Sender<SdkEvent>,
    text_tx: &'a mpsc::Sender<String>,
    voice_tx: &'a mpsc::Sender<VoiceEvent>,
    audio_tx: &'a mpsc::Sender<super::voice::AudioChunk>,
    transcript_tx: &'a mpsc::Sender<super::voice::TranscriptChunk>,
    active_response_id: &'a Arc<Mutex<Option<String>>>,
    auto_barge_in: bool,
    auto_tool_response: bool,
}

async fn handle_server_event(
    evt: ServerEvent,
    ctx: &mut EventContext<'_>,
    transport: &mut Box<dyn Transport>,
) {
    handle_voice_events(&evt, ctx, transport).await;
    handle_lifecycle_events(&evt, ctx).await;
    handle_user_transcript_events(&evt, ctx).await;

    if let Some(mapped) = SdkEvent::from_server(evt.clone()) {
        let _ = ctx.event_tx.send(mapped).await;
    }
    if let Some(handler) = &ctx.handlers.on_raw_event {
        let _ = handler(evt.clone()).await;
    }

    match evt {
        ServerEvent::ResponseOutputTextDelta {
            item_id,
            content_index,
            delta,
            ..
        } => {
            let key = (item_id, content_index);
            let entry = ctx.buffers.entry(key).or_default();
            entry.push_str(&delta);
        }
        ServerEvent::ResponseOutputTextDone {
            item_id,
            content_index,
            text,
            ..
        } => {
            let key = (item_id, content_index);
            ctx.buffers.remove(&key);
            let _ = ctx.text_tx.send(text.clone()).await;
            if let Some(handler) = &ctx.handlers.on_text {
                let _ = handler(text).await;
            }
        }
        ServerEvent::ResponseFunctionCallArgumentsDone {
            response_id,
            item_id,
            output_index,
            call_id,
            name,
            arguments,
            ..
        } => {
            let arguments =
                serde_json::from_str(&arguments).unwrap_or(serde_json::Value::String(arguments));
            let call = ToolCall {
                name,
                call_id: call_id.clone(),
                arguments,
                response_id: Some(response_id),
                item_id: Some(item_id),
                output_index: Some(output_index),
            };

            let result = if let Some(handler) = &ctx.handlers.on_tool_call {
                handler(call).await
            } else {
                ctx.dispatcher.dispatch(call).await
            };

            match result {
                Ok(tool_result) => {
                    let output = serde_json::to_string(&tool_result.output)
                        .unwrap_or_else(|_| String::new());
                    let item = Item::FunctionCallOutput {
                        id: None,
                        call_id: tool_result.call_id,
                        output,
                    };
                    let event = ClientEvent::ConversationItemCreate {
                        event_id: None,
                        previous_item_id: None,
                        item: Box::new(item),
                    };
                    let _ = transport.send(event).await;
                    if ctx.auto_tool_response {
                        let follow_up = ClientEvent::ResponseCreate {
                            event_id: None,
                            response: None,
                        };
                        let _ = transport.send(follow_up).await;
                    }
                }
                Err(err) => {
                    let output = serde_json::json!({ "error": err.to_string() }).to_string();
                    let item = Item::FunctionCallOutput {
                        id: None,
                        call_id,
                        output,
                    };
                    let event = ClientEvent::ConversationItemCreate {
                        event_id: None,
                        previous_item_id: None,
                        item: Box::new(item),
                    };
                    let _ = transport.send(event).await;
                }
            }
        }
        _ => {}
    }
}

async fn handle_lifecycle_events(evt: &ServerEvent, ctx: &EventContext<'_>) {
    match evt {
        ServerEvent::ResponseCreated { response, .. } => {
            {
                let mut guard = ctx.active_response_id.lock().await;
                *guard = Some(response.id.clone());
            }
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::ResponseCreated {
                    response_id: response.id.clone(),
                })
                .await;
        }
        ServerEvent::ResponseDone { response, .. } => {
            {
                let mut guard = ctx.active_response_id.lock().await;
                *guard = None;
            }
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::ResponseDone {
                    response_id: response.id.clone(),
                })
                .await;
        }
        ServerEvent::ResponseCancelled { response, .. } => {
            {
                let mut guard = ctx.active_response_id.lock().await;
                *guard = None;
            }
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::ResponseCancelled {
                    response_id: response.id.clone(),
                })
                .await;
        }
        _ => {}
    }
}

async fn handle_user_transcript_events(evt: &ServerEvent, ctx: &EventContext<'_>) {
    if let ServerEvent::InputAudioTranscriptionCompleted {
        item_id,
        content_index,
        transcript,
        ..
    } = evt
    {
        let _ = ctx
            .voice_tx
            .send(VoiceEvent::UserTranscriptDone {
                item_id: item_id.clone(),
                content_index: *content_index,
                transcript: transcript.clone(),
            })
            .await;
    }
}

async fn handle_voice_events(
    evt: &ServerEvent,
    ctx: &EventContext<'_>,
    transport: &mut Box<dyn Transport>,
) {
    handle_speech_events(evt, ctx, transport).await;
    handle_audio_events(evt, ctx).await;
    handle_transcript_events(evt, ctx).await;
}

async fn handle_speech_events(
    evt: &ServerEvent,
    ctx: &EventContext<'_>,
    transport: &mut Box<dyn Transport>,
) {
    match evt {
        ServerEvent::InputAudioBufferSpeechStarted { audio_start_ms, .. } => {
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::SpeechStarted {
                    audio_start_ms: Some(*audio_start_ms),
                })
                .await;
            if ctx.auto_barge_in {
                send_barge_in(ctx, transport).await;
            }
        }
        ServerEvent::InputAudioBufferSpeechStopped { audio_end_ms, .. } => {
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::SpeechStopped {
                    audio_end_ms: Some(*audio_end_ms),
                })
                .await;
        }
        _ => {}
    }
}

async fn handle_audio_events(evt: &ServerEvent, ctx: &EventContext<'_>) {
    match evt {
        ServerEvent::ResponseOutputAudioDelta {
            response_id,
            item_id,
            output_index,
            content_index,
            delta,
            ..
        } => {
            if !should_accept_response(ctx.active_response_id, response_id).await {
                return;
            }
            match general_purpose::STANDARD.decode(delta.as_bytes()) {
                Ok(pcm) => {
                    let _ = ctx
                        .voice_tx
                        .send(VoiceEvent::AudioDelta {
                            response_id: response_id.clone(),
                            item_id: item_id.clone(),
                            output_index: *output_index,
                            content_index: *content_index,
                            pcm: pcm.clone(),
                        })
                        .await;
                    let _ = ctx
                        .audio_tx
                        .send(super::voice::AudioChunk {
                            response_id: response_id.clone(),
                            item_id: item_id.clone(),
                            output_index: *output_index,
                            content_index: *content_index,
                            pcm,
                        })
                        .await;
                }
                Err(err) => {
                    let _ = ctx
                        .voice_tx
                        .send(VoiceEvent::DecodeError {
                            message: err.to_string(),
                        })
                        .await;
                }
            }
        }
        ServerEvent::ResponseOutputAudioDone {
            response_id,
            item_id,
            output_index,
            content_index,
            ..
        } => {
            if !should_accept_response(ctx.active_response_id, response_id).await {
                return;
            }
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::AudioDone {
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                })
                .await;
        }
        _ => {}
    }
}

async fn handle_transcript_events(evt: &ServerEvent, ctx: &EventContext<'_>) {
    match evt {
        ServerEvent::ResponseOutputAudioTranscriptDelta {
            response_id,
            item_id,
            output_index,
            content_index,
            delta,
            ..
        } => {
            if !should_accept_response(ctx.active_response_id, response_id).await {
                return;
            }
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::TranscriptDelta {
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    delta: delta.clone(),
                })
                .await;
            let _ = ctx
                .transcript_tx
                .send(super::voice::TranscriptChunk {
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    text: delta.clone(),
                    is_final: false,
                })
                .await;
        }
        ServerEvent::ResponseOutputAudioTranscriptDone {
            response_id,
            item_id,
            output_index,
            content_index,
            transcript,
            ..
        } => {
            if !should_accept_response(ctx.active_response_id, response_id).await {
                return;
            }
            let _ = ctx
                .voice_tx
                .send(VoiceEvent::TranscriptDone {
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    transcript: transcript.clone(),
                })
                .await;
            let _ = ctx
                .transcript_tx
                .send(super::voice::TranscriptChunk {
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    text: transcript.clone(),
                    is_final: true,
                })
                .await;
        }
        _ => {}
    }
}

async fn should_accept_response(active: &Arc<Mutex<Option<String>>>, response_id: &str) -> bool {
    let guard = active.lock().await;
    guard
        .as_deref()
        .is_none_or(|active_id| active_id == response_id)
}

async fn send_barge_in(ctx: &EventContext<'_>, transport: &mut Box<dyn Transport>) {
    let response_id = {
        let mut guard = ctx.active_response_id.lock().await;
        guard.take()
    };
    let _ = transport
        .send(ClientEvent::OutputAudioBufferClear { event_id: None })
        .await;
    if let Some(id) = response_id {
        let _ = transport
            .send(ClientEvent::ResponseCancel {
                event_id: None,
                response_id: Some(id),
            })
            .await;
    }
}

impl SessionHandle {
    /// Send a user text message.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn say(&self, text: impl Into<String>) -> Result<()> {
        let item = Item::Message {
            id: None,
            status: None,
            role: crate::protocol::models::Role::User,
            content: vec![ContentPart::InputText { text: text.into() }],
        };

        let event = ClientEvent::ConversationItemCreate {
            event_id: None,
            previous_item_id: None,
            item: Box::new(item),
        };

        self.send_event(event).await
    }

    /// Clear output audio and cancel any active response (barge-in).
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn barge_in(&self) -> Result<()> {
        self.send_event(ClientEvent::OutputAudioBufferClear { event_id: None })
            .await?;
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(Command::GetActiveResponseId { respond: tx })
            .await
            .map_err(|_| Error::ConnectionClosed)?;
        let response_id = rx.await.map_err(|_| Error::ConnectionClosed)?;

        if let Some(id) = response_id {
            self.send_event(ClientEvent::ResponseCancel {
                event_id: None,
                response_id: Some(id),
            })
            .await?;
        }
        Ok(())
    }

    /// Send raw PCM16 bytes to the input buffer.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn send_audio_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let encoded = general_purpose::STANDARD.encode(bytes);
        self.send_event(ClientEvent::InputAudioBufferAppend {
            event_id: None,
            audio: encoded,
        })
        .await?;
        self.send_event(ClientEvent::InputAudioBufferCommit { event_id: None })
            .await
    }

    /// Send PCM16 samples (i16) to the input buffer.
    ///
    /// # Errors
    /// Returns an error if encoding or send fails.
    pub async fn send_audio_pcm16(&self, samples: &[i16]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            buf.extend_from_slice(&sample.to_le_bytes());
        }
        self.send_audio_bytes(&buf).await
    }

    /// Send a raw protocol event.
    ///
    /// # Errors
    /// Returns an error if the send fails.
    pub async fn send_raw(&self, event: ClientEvent) -> Result<()> {
        self.send_event(event).await
    }

    async fn send_event(&self, event: ClientEvent) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(Command::SendWithResponse { event, respond: tx })
            .await
            .map_err(|_| Error::ConnectionClosed)?;
        rx.await.map_err(|_| Error::ConnectionClosed)?
    }
}

enum Command {
    SendWithResponse {
        event: ClientEvent,
        respond: oneshot::Sender<Result<()>>,
    },
    RunTool {
        call: ToolCall,
        respond: oneshot::Sender<Result<ToolResult>>,
    },
    GetActiveResponseId {
        respond: oneshot::Sender<Option<String>>,
    },
}

pub struct SessionConfigSnapshot {
    pub api_key: String,
    pub model: Option<String>,
    pub call_id: Option<String>,
    pub session: SessionConfig,
    pub handlers: EventHandlers,
    pub dispatcher: Arc<dyn ToolDispatcher>,
    pub auto_barge_in: bool,
    pub auto_tool_response: bool,
    pub send_initial_session_update: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SessionConnectTarget {
    Model(String),
    CallId(String),
}

impl SessionConfigSnapshot {
    #[must_use]
    pub(super) fn connection_target(&self) -> SessionConnectTarget {
        self.call_id.clone().map_or_else(
            || {
                SessionConnectTarget::Model(
                    self.model
                        .clone()
                        .unwrap_or_else(|| crate::protocol::models::DEFAULT_MODEL.to_string()),
                )
            },
            SessionConnectTarget::CallId,
        )
    }

    async fn connect_with_transport(self, transport: Box<dyn Transport>) -> Result<Session> {
        let session = Session::from_transport(
            transport,
            self.handlers,
            self.dispatcher,
            self.auto_barge_in,
            self.auto_tool_response,
        );
        if self.send_initial_session_update {
            let update = session_update_from_config(&self.session);
            session.update_session(update).await?;
        }
        Ok(session)
    }

    /// Connect via WebSocket.
    ///
    /// # Errors
    /// Returns an error if the connection fails.
    pub async fn connect_ws(self) -> Result<Session> {
        let target = self.connection_target();
        let client = match &target {
            SessionConnectTarget::Model(model) => {
                crate::RealtimeClient::connect(&self.api_key, Some(model.as_str()), None).await?
            }
            SessionConnectTarget::CallId(call_id) => {
                crate::RealtimeClient::connect(&self.api_key, None, Some(call_id.as_str())).await?
            }
        };

        let transport = Box::new(WsTransport { client });
        self.connect_with_transport(transport).await
    }
}

fn session_update_from_config(config: &SessionConfig) -> SessionUpdate {
    SessionUpdate {
        config: SessionUpdateConfig {
            kind: Some(config.kind),
            output_modalities: Some(config.output_modalities),
            modalities: config.modalities.clone(),
            include: config.include.clone(),
            prompt: config.prompt.clone(),
            truncation: config.truncation.clone(),
            instructions: config.instructions.clone(),
            input_audio_format: config.input_audio_format.clone(),
            output_audio_format: config.output_audio_format.clone(),
            input_audio_transcription: config.input_audio_transcription.clone(),
            turn_detection: config.turn_detection.clone(),
            tools: config.tools.clone(),
            tool_choice: config.tool_choice.clone(),
            temperature: config.temperature,
            max_output_tokens: config.max_output_tokens.clone(),
            audio: config.audio.clone(),
            tracing: config.tracing.clone(),
            voice: config.voice.clone(),
        },
    }
}

struct WsTransport {
    client: crate::RealtimeClient,
}

impl Transport for WsTransport {
    fn send(&mut self, event: ClientEvent) -> super::transport::BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.client.send(event).await })
    }

    fn next_event(&mut self) -> super::transport::BoxFuture<'_, Result<Option<ServerEvent>>> {
        Box::pin(async move { self.client.next_event().await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolRegistry;
    use crate::protocol::server_events::ServerEvent;
    use base64::engine::general_purpose;
    use futures::StreamExt;
    use tokio::sync::mpsc;

    struct MockTransport {
        incoming: mpsc::Receiver<ServerEvent>,
        outgoing: mpsc::Sender<ClientEvent>,
    }

    impl Transport for MockTransport {
        fn send(
            &mut self,
            event: ClientEvent,
        ) -> super::super::transport::BoxFuture<'_, Result<()>> {
            let outgoing = self.outgoing.clone();
            Box::pin(async move {
                outgoing
                    .send(event)
                    .await
                    .map_err(|_| Error::ConnectionClosed)?;
                Ok(())
            })
        }

        fn next_event(
            &mut self,
        ) -> super::super::transport::BoxFuture<'_, Result<Option<ServerEvent>>> {
            Box::pin(async move { Ok(self.incoming.recv().await) })
        }
    }

    #[tokio::test]
    async fn tool_call_sends_output() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let mut tools = ToolRegistry::new();
        tools.tool("echo", |args: serde_json::Value| async move { Ok(args) });

        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let evt = ServerEvent::ResponseFunctionCallArgumentsDone {
            event_id: "evt_1".to_string(),
            response_id: "resp_1".to_string(),
            item_id: "item_1".to_string(),
            output_index: 0,
            call_id: "call_1".to_string(),
            name: "echo".to_string(),
            arguments: r#"{"hello":"world"}"#.to_string(),
        };

        event_tx.send(evt).await.unwrap();

        let sent = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match sent {
            ClientEvent::ConversationItemCreate { item, .. } => match *item {
                Item::FunctionCallOutput {
                    call_id, output, ..
                } => {
                    assert_eq!(call_id, "call_1");
                    assert!(output.contains("hello"));
                }
                other => panic!("unexpected item: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }

        let follow_up = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(follow_up, ClientEvent::ResponseCreate { .. }));

        drop(session);
    }

    #[tokio::test]
    async fn next_event_maps_sdk_event() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let evt = ServerEvent::ResponseOutputTextDelta {
            event_id: "evt_1".to_string(),
            response_id: "resp_1".to_string(),
            item_id: "item_1".to_string(),
            output_index: 0,
            content_index: 0,
            delta: "hello".to_string(),
        };
        event_tx.send(evt).await.unwrap();

        let mapped = session.next_event().await.unwrap().expect("sdk event");
        match mapped {
            SdkEvent::TextDelta { delta, .. } => assert_eq!(delta, "hello"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn event_stream_yields_sdk_event() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let evt = ServerEvent::ResponseOutputTextDone {
            event_id: "evt_1".to_string(),
            response_id: "resp_1".to_string(),
            item_id: "item_1".to_string(),
            output_index: 0,
            content_index: 0,
            text: "done".to_string(),
        };
        event_tx.send(evt).await.unwrap();

        let mut stream = session.events();
        let mapped = stream.next().await.expect("sdk event");
        match mapped {
            SdkEvent::TextDone { text, .. } => assert_eq!(text, "done"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_response_emits_response_create() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let config = crate::protocol::models::ResponseConfig {
            instructions: Some("Respond.".to_string()),
            ..Default::default()
        };

        session.send_response(config).await.unwrap();

        let sent = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match sent {
            ClientEvent::ResponseCreate { response, .. } => {
                let response = response.expect("response config");
                assert_eq!(response.instructions.as_deref(), Some("Respond."));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        drop(event_tx);
    }

    #[tokio::test]
    async fn approve_mcp_sends_item() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        session.approve_mcp("req_1", Some("ok")).await.unwrap();

        let sent = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match sent {
            ClientEvent::ConversationItemCreate { item, .. } => match *item {
                Item::McpApprovalResponse {
                    approval_request_id,
                    approve,
                    reason,
                    ..
                } => {
                    assert_eq!(approval_request_id, "req_1");
                    assert!(approve);
                    assert_eq!(reason.as_deref(), Some("ok"));
                }
                other => panic!("unexpected item: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }

        drop(event_tx);
    }

    #[tokio::test]
    async fn ask_sends_and_returns_text() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let event_tx_clone = event_tx.clone();
        let send_evt = async move {
            let evt = ServerEvent::ResponseOutputTextDone {
                event_id: "evt_1".to_string(),
                response_id: "resp_1".to_string(),
                item_id: "item_1".to_string(),
                output_index: 0,
                content_index: 0,
                text: "hello".to_string(),
            };
            event_tx_clone.send(evt).await.unwrap();
        };
        tokio::spawn(send_evt);

        let text = session.ask("hi").await.unwrap().expect("text");
        assert_eq!(text, "hello");

        // Ensure we sent both the item and the response.create.
        let first = out_rx.recv().await.unwrap();
        let second = out_rx.recv().await.unwrap();
        assert!(
            matches!(first, ClientEvent::ConversationItemCreate { .. })
                || matches!(second, ClientEvent::ConversationItemCreate { .. })
        );
        assert!(
            matches!(first, ClientEvent::ResponseCreate { .. })
                || matches!(second, ClientEvent::ResponseCreate { .. })
        );

        drop(event_tx);
    }

    #[tokio::test]
    async fn voice_event_audio_delta_decodes() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let pcm = vec![1u8, 2u8, 3u8, 4u8];
        let delta = general_purpose::STANDARD.encode(&pcm);
        let evt = ServerEvent::ResponseOutputAudioDelta {
            event_id: "evt_1".to_string(),
            response_id: "resp_1".to_string(),
            item_id: "item_1".to_string(),
            output_index: 0,
            content_index: 0,
            delta,
        };
        event_tx.send(evt).await.unwrap();

        let voice = session
            .next_voice_event()
            .await
            .unwrap()
            .expect("voice event");
        match voice {
            VoiceEvent::AudioDelta {
                response_id,
                pcm: decoded,
                ..
            } => {
                assert_eq!(response_id, "resp_1");
                assert_eq!(decoded, pcm);
            }
            other => panic!("unexpected voice event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn voice_event_audio_done_propagates_response_id() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let evt = ServerEvent::ResponseOutputAudioDone {
            event_id: "evt_2".to_string(),
            response_id: "resp_42".to_string(),
            item_id: "item_2".to_string(),
            output_index: 1,
            content_index: 0,
            item: None,
        };
        event_tx.send(evt).await.unwrap();

        let voice = session
            .next_voice_event()
            .await
            .unwrap()
            .expect("voice event");
        match voice {
            VoiceEvent::AudioDone {
                response_id,
                item_id,
                output_index,
                content_index,
            } => {
                assert_eq!(response_id, "resp_42");
                assert_eq!(item_id, "item_2");
                assert_eq!(output_index, 1);
                assert_eq!(content_index, 0);
            }
            other => panic!("unexpected voice event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_audio_pcm16_appends_and_commits() {
        let (_event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let pcm = vec![0i16; 4];
        session.send_audio_pcm16(&pcm).await.unwrap();

        let first = out_rx.recv().await.unwrap();
        let second = out_rx.recv().await.unwrap();

        assert!(
            matches!(first, ClientEvent::InputAudioBufferAppend { .. })
                || matches!(second, ClientEvent::InputAudioBufferAppend { .. })
        );
        assert!(
            matches!(first, ClientEvent::InputAudioBufferCommit { .. })
                || matches!(second, ClientEvent::InputAudioBufferCommit { .. })
        );
    }

    #[tokio::test]
    async fn audio_handle_push_and_commit() {
        let (_event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let pcm = vec![0i16; 4];
        session.audio().push_pcm16(&pcm).await.unwrap();
        session.audio().commit().await.unwrap();

        let first = out_rx.recv().await.unwrap();
        let second = out_rx.recv().await.unwrap();

        assert!(
            matches!(first, ClientEvent::InputAudioBufferAppend { .. })
                || matches!(second, ClientEvent::InputAudioBufferAppend { .. })
        );
        assert!(
            matches!(first, ClientEvent::InputAudioBufferCommit { .. })
                || matches!(second, ClientEvent::InputAudioBufferCommit { .. })
        );
    }

    #[tokio::test]
    async fn stream_audio_pcm16_sends_chunks() {
        let (_event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let stream = futures::stream::iter(vec![vec![0i16; 2], vec![1i16; 2]]);
        session.stream_audio_pcm16(stream).await.unwrap();

        let mut saw_append = 0;
        let mut saw_commit = 0;
        for _ in 0..4 {
            let evt = out_rx.recv().await.unwrap();
            match evt {
                ClientEvent::InputAudioBufferAppend { .. } => saw_append += 1,
                ClientEvent::InputAudioBufferCommit { .. } => saw_commit += 1,
                _ => {}
            }
        }
        assert_eq!(saw_append, 2);
        assert_eq!(saw_commit, 2);
    }

    #[tokio::test]
    async fn barge_in_sends_clear_and_cancel() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let resp = crate::protocol::models::Response {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            conversation_id: None,
            status: crate::protocol::models::ResponseStatus::InProgress,
            status_details: None,
            output: None,
            output_modalities: None,
            max_output_tokens: None,
            audio: None,
            metadata: None,
            usage: None,
        };
        let evt = ServerEvent::ResponseCreated {
            event_id: "evt_1".to_string(),
            response: resp,
        };
        event_tx.send(evt).await.unwrap();

        let _ = session.next_voice_event().await.unwrap();
        session.barge_in().await.unwrap();

        let first = out_rx.recv().await.unwrap();
        let second = out_rx.recv().await.unwrap();

        assert!(
            matches!(first, ClientEvent::OutputAudioBufferClear { .. })
                || matches!(second, ClientEvent::OutputAudioBufferClear { .. })
        );
        assert!(
            matches!(first, ClientEvent::ResponseCancel { .. })
                || matches!(second, ClientEvent::ResponseCancel { .. })
        );
    }

    #[tokio::test]
    async fn auto_barge_in_on_speech_started() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session =
            Session::from_transport(transport, EventHandlers::new(), Arc::new(tools), true, true);

        let resp = crate::protocol::models::Response {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            conversation_id: None,
            status: crate::protocol::models::ResponseStatus::InProgress,
            status_details: None,
            output: None,
            output_modalities: None,
            max_output_tokens: None,
            audio: None,
            metadata: None,
            usage: None,
        };
        let created = ServerEvent::ResponseCreated {
            event_id: "evt_1".to_string(),
            response: resp,
        };
        event_tx.send(created).await.unwrap();
        let _ = session.next_voice_event().await.unwrap();

        let speech = ServerEvent::InputAudioBufferSpeechStarted {
            event_id: "evt_2".to_string(),
            audio_start_ms: 0,
            item_id: "item_1".to_string(),
        };
        event_tx.send(speech).await.unwrap();
        let _ = session.next_voice_event().await.unwrap();

        let first = out_rx.recv().await.unwrap();
        let second = out_rx.recv().await.unwrap();

        assert!(
            matches!(first, ClientEvent::OutputAudioBufferClear { .. })
                || matches!(second, ClientEvent::OutputAudioBufferClear { .. })
        );
        assert!(
            matches!(first, ClientEvent::ResponseCancel { .. })
                || matches!(second, ClientEvent::ResponseCancel { .. })
        );
    }

    #[tokio::test]
    async fn audio_deltas_gate_on_active_response() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let resp = crate::protocol::models::Response {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            conversation_id: None,
            status: crate::protocol::models::ResponseStatus::InProgress,
            status_details: None,
            output: None,
            output_modalities: None,
            max_output_tokens: None,
            audio: None,
            metadata: None,
            usage: None,
        };
        event_tx
            .send(ServerEvent::ResponseCreated {
                event_id: "evt_1".to_string(),
                response: resp,
            })
            .await
            .unwrap();
        let _ = session.next_voice_event().await.unwrap();

        let pcm = vec![1u8, 2u8];
        let delta = general_purpose::STANDARD.encode(&pcm);
        let evt = ServerEvent::ResponseOutputAudioDelta {
            event_id: "evt_2".to_string(),
            response_id: "resp_2".to_string(),
            item_id: "item_1".to_string(),
            output_index: 0,
            content_index: 0,
            delta,
        };
        event_tx.send(evt).await.unwrap();

        // Should not receive audio chunk for different response_id.
        let chunk = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            session.next_audio_chunk(),
        )
        .await;
        assert!(chunk.is_err());
    }

    #[tokio::test]
    async fn session_loop_exits_when_sender_closed() {
        let (_event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        drop(session);

        let closed = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
            .await
            .expect("session loop did not exit");
        assert!(closed.is_none());
    }

    #[tokio::test]
    async fn user_transcript_event_includes_content_index() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, _out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let tools = ToolRegistry::new();
        let mut session = Session::from_transport(
            transport,
            EventHandlers::new(),
            Arc::new(tools),
            false,
            true,
        );

        let evt = ServerEvent::InputAudioTranscriptionCompleted {
            event_id: "evt_1".to_string(),
            item_id: "item_1".to_string(),
            content_index: 2,
            transcript: "hello".to_string(),
            logprobs: None,
            usage: None,
        };
        event_tx.send(evt).await.unwrap();

        let voice = session
            .next_voice_event()
            .await
            .unwrap()
            .expect("voice event");
        match voice {
            VoiceEvent::UserTranscriptDone {
                item_id,
                content_index,
                transcript,
            } => {
                assert_eq!(item_id, "item_1");
                assert_eq!(content_index, 2);
                assert_eq!(transcript, "hello");
            }
            other => panic!("unexpected voice event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn manual_sideband_connect_skips_initial_session_update() {
        let (_event_tx, event_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let transport = Box::new(MockTransport {
            incoming: event_rx,
            outgoing: out_tx,
        });

        let snapshot = SessionConfigSnapshot {
            api_key: "test-key".to_string(),
            model: Some("gpt-realtime".to_string()),
            call_id: Some("call_123".to_string()),
            session: SessionConfig::new(
                crate::protocol::models::SessionKind::Realtime,
                "gpt-realtime".to_string(),
                crate::protocol::models::OutputModalities::Audio,
            ),
            handlers: EventHandlers::new(),
            dispatcher: Arc::new(ToolRegistry::new()),
            auto_barge_in: false,
            auto_tool_response: false,
            send_initial_session_update: false,
        };

        let session = snapshot
            .connect_with_transport(transport)
            .await
            .expect("manual sideband session");

        let initial =
            tokio::time::timeout(std::time::Duration::from_millis(100), out_rx.recv()).await;
        assert!(
            initial.is_err(),
            "manual sideband connect should not auto-send session.update"
        );

        session.respond().await.expect("manual response.create");
        let sent = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(sent, ClientEvent::ResponseCreate { .. }));
    }
}
