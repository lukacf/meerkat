//! Public OpenAI Live (`gpt-live-1`) broker adapter over `oai_rt_rs::live`.
//!
//! This module owns provider mechanics only. It validates a registry-minted
//! realtime target for the public Live API, creates the WebRTC session with a
//! client delegation, attaches the server-side sideband, and lowers public
//! Live events to the shared [`GptLiveBrokerObservation`] vocabulary consumed
//! by the provider-neutral facade.
//!
//! The public Live API has no authoritative turn identifiers, completed-turn
//! events, or delegation task text. This adapter therefore synthesizes turns
//! from transcript role alternation and joins a client delegation to the most
//! recent user turn. Those joins are provider evidence only; transcript
//! admission, delegation meaning, channel policy, and model selection remain
//! outside this module.

use std::collections::{HashSet, VecDeque};

use meerkat_core::model_profile::catalog::ModelReleaseStage;
use meerkat_core::types::Message;
use meerkat_llm_core::provider_runtime::errors::ProviderClientError;
use meerkat_llm_core::provider_runtime::{NormalizedBackendKind, ResolvedRealtimeTarget};
use oai_rt_rs::live::{
    AudioConfig, AudioOutput, ClientEvent, ClientOptions, Command, CreateRequest, Delegation,
    DelegationConfig, DelegationTarget, DelegationType, Error as LiveError, Field, InitialItem,
    InitialRole, InitialText, InitialTextType, LiveClient, LiveReceiver, LiveSender, MessageType,
    Nullable, ServerEvent, ServerFrame, SessionConfig, Voice, WebRtcTransport,
};
use tokio::sync::Mutex;

use crate::OpenAiBackendKind;
use crate::gpt_live_broker::{
    GptLiveAppendToken, GptLiveBrokerError, GptLiveBrokerObservation, GptLiveBrokerTerminalClass,
    GptLiveDelegationRef, GptLiveDelegationTarget, GptLiveTranscriptItemRef, GptLiveTurnRef,
    GptLiveTurnRole, protocol_error, require_context, summarize_unknown_provider_event,
};

pub use crate::runtime::GPT_LIVE_MODEL_FAMILY;

/// Provider-owned mechanical configuration for one browser WebRTC bootstrap.
///
/// The public broker always starts the session with a client delegation: the
/// voice model speaks and the channel-bound Meerkat executor performs work.
#[derive(Clone)]
pub struct PublicLiveOpenConfig {
    offer_sdp: String,
    voice: String,
    instructions: Option<String>,
    input: Vec<InitialItem>,
}

impl PublicLiveOpenConfig {
    /// Construct the minimum verified public session shape.
    ///
    /// # Errors
    ///
    /// Returns a typed local validation error for blank SDP or voice input.
    pub fn new(
        offer_sdp: impl Into<String>,
        voice: impl Into<String>,
    ) -> Result<Self, GptLiveBrokerError> {
        let offer_sdp = offer_sdp.into();
        if offer_sdp.trim().is_empty() {
            return Err(GptLiveBrokerError::MissingOfferSdp);
        }
        let voice = voice.into();
        if voice.trim().is_empty() {
            return Err(GptLiveBrokerError::MissingVoice);
        }
        if !oai_rt_rs::live::VOICES.contains(&voice.as_str()) {
            // Public voice names are extensible and custom voices exist, so
            // this is not a rejection; the provider answers an unknown name
            // with HTTP 403 "Voice session access denied", which this note
            // makes diagnosable without exposing the configured value.
            tracing::warn!(
                known_voices = ?oai_rt_rs::live::VOICES,
                "public Live voice is not one of the released public voice names"
            );
        }
        Ok(Self {
            offer_sdp,
            voice,
            instructions: None,
            input: Vec::new(),
        })
    }

    /// Lower host-catalog guidance into the startup-only `session.instructions`.
    /// Raw `live/open` prose never reaches this seam.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Replay the selected canonical dialogue as startup history, not as a
    /// speech-prompting commentary append. Executor instructions and tool
    /// mechanics do not belong to the tool-less voice model's conversation.
    ///
    /// No history is truncated here. The provider's startup limits are
    /// validated by the protocol client and provider; oversize seeds reject
    /// channel creation rather than silently losing canonical context.
    #[must_use]
    pub fn with_history(mut self, messages: &[Message]) -> Self {
        self.input = messages
            .iter()
            .filter_map(|message| {
                let (role, text_type, text) = match message {
                    Message::User(user) => (
                        InitialRole::User,
                        InitialTextType::InputText,
                        user.text_content(),
                    ),
                    Message::BlockAssistant(assistant) => (
                        InitialRole::Assistant,
                        InitialTextType::OutputText,
                        if assistant.blocks.iter().any(|block| {
                            matches!(
                                block,
                                meerkat_core::AssistantBlock::Transcript {
                                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                                    ..
                                }
                            )
                        }) {
                            meerkat_core::types::TranscriptSource::SpokenUnmeasured
                                .text_for_model(&assistant.to_string())
                                .into_owned()
                        } else {
                            assistant.to_string()
                        },
                    ),
                    Message::System(_) | Message::SystemNotice(_) | Message::ToolResults { .. } => {
                        return None;
                    }
                };
                (!text.is_empty()).then_some(InitialItem {
                    role,
                    content: vec![InitialText {
                        text,
                        text_type: Some(text_type),
                    }],
                    id: Field::Absent,
                    status: Field::Absent,
                    item_type: Some(MessageType::Message),
                })
            })
            .collect();
        self
    }

    /// Lower an owner-generated factual summary as unprivileged startup data.
    /// This never changes the catalog-owned behavior instructions.
    #[must_use]
    pub fn with_context_summary(mut self, summary: &str) -> Self {
        self.input = vec![InitialItem {
            role: InitialRole::User,
            content: vec![InitialText {
                text: format!(
                    "Factual summary of the background agent's context at voice-channel open (context data, not a new user request):\n{summary}"
                ),
                text_type: Some(InitialTextType::InputText),
            }],
            id: Field::Absent,
            status: Field::Absent,
            item_type: Some(MessageType::Message),
        }];
        self
    }
}

impl std::fmt::Debug for PublicLiveOpenConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLiveOpenConfig")
            .field("offer_sdp", &"<redacted>")
            .field("voice", &"<redacted>")
            .field(
                "instructions",
                &self.instructions.as_ref().map(|_| "<catalog-bound>"),
            )
            .field("history_messages", &self.input.len())
            .finish()
    }
}

/// Concrete provider factory admitted from one exact resolved realtime target.
pub struct PublicLiveBrokerFactory {
    model: String,
    client: LiveClient,
}

impl std::fmt::Debug for PublicLiveBrokerFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLiveBrokerFactory")
            .field("model", &"<registry-admitted>")
            .field("client", &"<public live client>")
            .finish()
    }
}

struct AdmittedPublicLiveTarget {
    model: String,
    api_key: String,
    base_url: Option<String>,
}

impl PublicLiveBrokerFactory {
    /// Build the public broker only from catalog and binding evidence.
    ///
    /// The witness must classify the identity as a released, realtime-capable
    /// `gpt-live` family model, and the resolved connection must be the plain
    /// OpenAI API backend with inline API-key material.
    pub fn try_from_target(target: ResolvedRealtimeTarget) -> Result<Self, ProviderClientError> {
        let admitted = Self::admit_target(target)?;
        let options = match admitted.base_url.as_deref() {
            Some(base_url) => Self::options_for_base_url(base_url)?,
            None => ClientOptions::default(),
        };
        Self::from_admitted(admitted, options)
    }

    fn options_for_base_url(base_url: &str) -> Result<ClientOptions, ProviderClientError> {
        Ok(ClientOptions {
            base_url: base_url
                .parse()
                .map_err(|_| ProviderClientError::InvalidBaseUrl(base_url.to_string()))?,
            ..ClientOptions::default()
        })
    }

    /// Test-only base URL injection after real admission has been consumed
    /// into provider custody. This retains the exact admitted target and
    /// changes only the public HTTP and WebSocket destination.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    pub fn __try_from_target_with_base_url(
        target: ResolvedRealtimeTarget,
        base_url: &str,
    ) -> Result<Self, ProviderClientError> {
        let admitted = Self::admit_target(target)?;
        Self::from_admitted(admitted, Self::options_for_base_url(base_url)?)
    }

    fn from_admitted(
        admitted: AdmittedPublicLiveTarget,
        options: ClientOptions,
    ) -> Result<Self, ProviderClientError> {
        let client = LiveClient::with_options(&admitted.api_key, options).map_err(|_| {
            ProviderClientError::ClientInit("failed to construct public Live client".to_string())
        })?;
        Ok(Self {
            model: admitted.model,
            client,
        })
    }

    fn admit_target(
        target: ResolvedRealtimeTarget,
    ) -> Result<AdmittedPublicLiveTarget, ProviderClientError> {
        let profile = target.profile().profile();
        if profile.model_family != GPT_LIVE_MODEL_FAMILY {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-model-family",
            ));
        }
        if profile.release_stage == ModelReleaseStage::Experimental {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-released-model",
            ));
        }
        if !profile.realtime {
            return Err(ProviderClientError::MissingFeature("openai-live-realtime"));
        }
        let (identity, _, connection) = target.into_parts();
        if !matches!(
            connection.backend,
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi)
        ) {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-openai-api-backend",
            ));
        }
        if connection.resolved_authorizer().is_some() {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-authorizer-auth",
            ));
        }
        let api_key = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        Ok(AdmittedPublicLiveTarget {
            model: identity.model,
            api_key,
            base_url: connection.backend_profile.base_url.clone(),
        })
    }

    /// Create the public WebRTC session and attach its sideband before
    /// returning.
    ///
    /// The answer SDP remains opaque browser bootstrap data. The returned
    /// session keeps the provider session identity and raw events inside the
    /// OpenAI adapter boundary.
    pub async fn open(
        &self,
        config: PublicLiveOpenConfig,
    ) -> Result<PublicLiveBootstrap, GptLiveBrokerError> {
        let request = CreateRequest {
            session: self.session_config(&config),
            transport: WebRtcTransport::WebRtc {
                sdp: config.offer_sdp,
            },
        };
        let created = self
            .client
            .create_webrtc(&request)
            .await
            .map_err(map_live_error)?;
        let answer_sdp = created.transport.sdp().to_string();
        let sideband = self
            .client
            .attach(&created.session.id)
            .await
            .map_err(map_live_error)?;
        let (sender, receiver) = sideband.split();
        Ok(PublicLiveBootstrap {
            answer_sdp,
            session: PublicLiveBrokerSession {
                sender,
                receiver: Mutex::new(receiver),
                state: Mutex::new(SessionState::default()),
            },
        })
    }

    fn session_config(&self, config: &PublicLiveOpenConfig) -> SessionConfig {
        SessionConfig {
            model: self.model.clone(),
            // WebRTC negotiates media; only the voice is selected here.
            audio: Some(AudioConfig {
                format: None,
                output: Some(AudioOutput {
                    voice: Some(Voice::Named(config.voice.clone())),
                }),
            }),
            client: None,
            delegation: Field::Value(DelegationConfig::Client),
            input: (!config.input.is_empty()).then(|| config.input.clone()),
            instructions: config
                .instructions
                .clone()
                .map_or(Field::Absent, Field::Value),
            store: None,
        }
    }
}

/// Browser-facing SDP answer paired with a provider-owned opaque broker session.
pub struct PublicLiveBootstrap {
    answer_sdp: String,
    session: PublicLiveBrokerSession,
}

impl std::fmt::Debug for PublicLiveBootstrap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLiveBootstrap")
            .field("answer_sdp", &"<redacted>")
            .field("session", &self.session)
            .finish()
    }
}

impl PublicLiveBootstrap {
    /// Borrow the answer SDP for the browser peer connection.
    #[must_use]
    pub fn answer_sdp(&self) -> &str {
        &self.answer_sdp
    }

    /// Transfer the opaque broker session to its provider-owned host.
    #[must_use]
    pub fn into_parts(self) -> (String, PublicLiveBrokerSession) {
        (self.answer_sdp, self.session)
    }
}

/// Opaque connected sideband handle for one public Live session.
///
/// Provider session identity and raw events are intentionally not exposed.
/// Events are lowered to sanitized observations inside this crate.
pub struct PublicLiveBrokerSession {
    sender: LiveSender,
    receiver: Mutex<LiveReceiver>,
    state: Mutex<SessionState>,
}

impl std::fmt::Debug for PublicLiveBrokerSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PublicLiveBrokerSession(<connected>)")
    }
}

impl PublicLiveBrokerSession {
    /// Seed one ordered canonical commentary envelope after the answer has
    /// been delivered and the sideband has attached. The exact commentary
    /// acknowledgement is the server-side readiness evidence when a seed
    /// exists. Valid observations that race ahead of the acknowledgement are
    /// preserved in order. Any missing, ambiguous, or mismatched
    /// acknowledgement closes the partial session; this method never retries.
    pub async fn await_ready_and_seed_session_context(
        &self,
        commentary: Option<String>,
    ) -> Result<(), GptLiveBrokerError> {
        let seeded = async {
            let Some(commentary) = commentary else {
                return Ok(());
            };
            let token = self.append_session_context(commentary).await?;
            let mut deferred = VecDeque::new();
            loop {
                match self.next_observation().await? {
                    Some(GptLiveBrokerObservation::SessionContextAppendAcknowledged {
                        token: acknowledged,
                    }) if acknowledged == token => break,
                    Some(GptLiveBrokerObservation::SessionContextAppendAcknowledged { .. }) => {
                        return Err(protocol_error());
                    }
                    Some(GptLiveBrokerObservation::UnsupportedProviderEvent) => {
                        return Err(protocol_error());
                    }
                    Some(observation) => deferred.push_back(observation),
                    None => {
                        return Err(GptLiveBrokerError::Transport {
                            class: GptLiveBrokerTerminalClass::Protocol,
                        });
                    }
                }
            }
            if !deferred.is_empty() {
                let mut state = self.state.lock().await;
                deferred.append(&mut state.queued_observations);
                state.queued_observations = deferred;
            }
            Ok(())
        }
        .await;
        if seeded.is_err() {
            let _ = self.close().await;
        }
        seeded
    }

    /// Append canonical Meerkat context as commentary without granting it
    /// automatic speech.
    ///
    /// Every append carries a `client_event_id`; the provider echoes it on the
    /// acknowledgement, which is how pending appends are correlated. An
    /// acknowledgement without an id is accepted only while exactly one append
    /// is pending; otherwise it is ambiguous and fails closed. A send failure
    /// is classified as ambiguous and fails every later append closed so an
    /// unacknowledged write can never be silently retried or misattributed.
    pub async fn append_session_context(
        &self,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = self
            .state
            .lock()
            .await
            .reserve_append(PendingAppendLane::Session)?;
        let event = Self::commentary_event(token, text, Nullable(None));
        self.deliver_append(token, event).await
    }

    /// Append executor context to an observed client delegation.
    ///
    /// The provider identifier remains inside the opaque delegation reference.
    /// Results and progress are always appended as commentary; the live model
    /// alone decides whether and how to speak from that context.
    pub async fn append_delegation_context(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = self
            .state
            .lock()
            .await
            .reserve_append(PendingAppendLane::Delegation)?;
        let event = Self::commentary_event(token, text, Nullable(Some(delegation.0.clone())));
        self.deliver_append(token, event).await
    }

    fn commentary_event(
        token: GptLiveAppendToken,
        content: String,
        delegation_id: Nullable<String>,
    ) -> ClientEvent {
        ClientEvent {
            event_id: Field::Value(pending_event_id(token)),
            command: Command::CommentaryAppend {
                content,
                delegation_id,
            },
        }
    }

    async fn deliver_append(
        &self,
        token: GptLiveAppendToken,
        event: ClientEvent,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        if self.sender.send(event).await.is_err() {
            self.state.lock().await.append_delivery_ambiguous = true;
            return Err(GptLiveBrokerError::AppendDeliveryAmbiguous { token });
        }
        Ok(token)
    }

    /// Receive and sanitize one sideband event.
    ///
    /// Turn boundaries are synthesized from transcript role alternation inside
    /// this serialized receive owner. A client delegation is joined to the
    /// open or most recent user turn and is the sole terminal observation for
    /// an open user turn. This is provider evidence only and grants no
    /// executor authority.
    pub async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
        let mut receiver = self.receiver.lock().await;
        loop {
            {
                let mut state = self.state.lock().await;
                if let Some(observation) = state.queued_observations.pop_front() {
                    tracing::debug!(?observation, "public Live lowered a sideband observation");
                    return Ok(Some(observation));
                }
            }
            // Missing transcript events are neither silence nor completion.
            // Keep the same output identity across pauses, including while a
            // delegation result is being injected into the provider context.
            let next = receiver.next_event().await;
            let Some(frame) = next.map_err(map_live_error)? else {
                if !self.state.lock().await.closed_observed {
                    return Err(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::WebSocket,
                    });
                }
                return Ok(None);
            };
            self.state.lock().await.apply_frame(frame)?;
        }
    }

    /// Request `session.close` through the sideband without exposing its
    /// wire identity. Successful repeated requests are idempotent. Continue
    /// draining observations: only `session.closed`, not this send or a bare
    /// transport EOF, confirms physical closure.
    pub async fn close(&self) -> Result<(), GptLiveBrokerError> {
        let mut state = self.state.lock().await;
        if state.close_requested || state.closed_observed {
            return Ok(());
        }
        self.sender
            .send(ClientEvent::new(Command::Close))
            .await
            .map_err(map_live_error)?;
        state.close_requested = true;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingAppendLane {
    Session,
    Delegation,
}

struct PendingAppend {
    lane: PendingAppendLane,
    token: GptLiveAppendToken,
}

struct OpenTurn {
    provider_ref: String,
    role: GptLiveTurnRole,
    /// Transcript deltas with their session-relative start offsets, so a
    /// delegation can freeze exactly the prefix observed before its offset.
    segments: Vec<TranscriptSegment>,
}

struct TranscriptSegment {
    start_ms: f64,
    text: String,
}

fn join_segments<'a>(segments: impl IntoIterator<Item = &'a TranscriptSegment>) -> String {
    segments
        .into_iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

struct FinishedUserTurn {
    transcript: String,
}

struct SessionState {
    next_append_token: u64,
    pending_appends: VecDeque<PendingAppend>,
    append_delivery_ambiguous: bool,
    next_turn: u64,
    next_transcript_item: u64,
    open_turn: Option<OpenTurn>,
    last_user_turn: Option<FinishedUserTurn>,
    seen_delegation_ids: HashSet<String>,
    queued_observations: VecDeque<GptLiveBrokerObservation>,
    reflected_output_audio_frames: u64,
    close_requested: bool,
    closed_observed: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            next_append_token: 1,
            pending_appends: VecDeque::new(),
            append_delivery_ambiguous: false,
            next_turn: 0,
            next_transcript_item: 0,
            open_turn: None,
            last_user_turn: None,
            seen_delegation_ids: HashSet::new(),
            queued_observations: VecDeque::new(),
            reflected_output_audio_frames: 0,
            close_requested: false,
            closed_observed: false,
        }
    }
}

impl SessionState {
    const MAX_DELEGATION_IDENTITIES: usize = 4096;
    const MAX_PENDING_APPENDS: usize = 64;

    fn reserve_append(
        &mut self,
        lane: PendingAppendLane,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        if self.append_delivery_ambiguous || self.pending_appends.len() >= Self::MAX_PENDING_APPENDS
        {
            return Err(GptLiveBrokerError::AppendInFlight);
        }
        let token = GptLiveAppendToken(self.next_append_token);
        self.next_append_token = self.next_append_token.saturating_add(1);
        self.pending_appends
            .push_back(PendingAppend { lane, token });
        Ok(token)
    }

    fn apply_frame(&mut self, frame: ServerFrame) -> Result<(), GptLiveBrokerError> {
        let ServerFrame {
            event,
            client_event_id,
            raw,
        } = frame;
        match event {
            // Readiness is a host-owned fact established by the seed
            // acknowledgement; a sideband `session.started` (or its replay)
            // is not projected as a second readiness observation.
            ServerEvent::Started { .. } | ServerEvent::Updated { .. } => {}
            ServerEvent::Closed { .. } => {
                // The SDK ends the stream after the terminal event; flush the
                // open turn so its final transcript is not lost.
                self.closed_observed = true;
                self.finish_open_turn();
            }
            ServerEvent::CommentaryAppended { .. } => {
                let pending = self.take_acknowledged_append(client_event_id.as_deref())?;
                self.queued_observations.push_back(match pending.lane {
                    PendingAppendLane::Session => {
                        GptLiveBrokerObservation::SessionContextAppendAcknowledged {
                            token: pending.token,
                        }
                    }
                    PendingAppendLane::Delegation => {
                        GptLiveBrokerObservation::DelegationContextAppendAcknowledged {
                            token: pending.token,
                        }
                    }
                });
            }
            // This broker never appends instructions or thinking; their
            // acknowledgements would indicate another controller on the
            // session and carry no observation for this channel.
            ServerEvent::InstructionsAppended { .. } | ServerEvent::ThinkingAppended { .. } => {
                tracing::debug!("public Live acknowledged an append this broker did not send");
            }
            ServerEvent::InputTranscriptDelta {
                delta, start_ms, ..
            } => {
                self.record_transcript_delta(GptLiveTurnRole::User, start_ms, delta);
            }
            ServerEvent::OutputTranscriptDelta {
                delta, start_ms, ..
            } => {
                self.record_transcript_delta(GptLiveTurnRole::Assistant, start_ms, delta);
            }
            ServerEvent::DelegationCreated {
                delegation,
                offset_ms,
                ..
            } => {
                self.record_delegation(delegation, offset_ms)?;
            }
            // Reflected media, mute state, accounting telemetry, telephony
            // signalling, and informational notices carry no conversational,
            // transcript, delegation, or effect authority.
            ServerEvent::OutputAudioDelta { .. } => {
                // Reflected media is not evidence of transcript or playback
                // completion; frames may also represent silence.
                self.reflected_output_audio_frames =
                    self.reflected_output_audio_frames.saturating_add(1);
                if self.reflected_output_audio_frames.is_multiple_of(50) {
                    tracing::debug!(
                        frames = self.reflected_output_audio_frames,
                        "public Live sideband reflected output audio"
                    );
                }
            }
            ServerEvent::InputAudio { .. }
            | ServerEvent::InputAudioMuted { .. }
            | ServerEvent::InputAudioUnmuted { .. }
            | ServerEvent::UsageUpdated { .. }
            | ServerEvent::Info { .. }
            | ServerEvent::DtmfReceived { .. }
            | ServerEvent::DtmfSend { .. }
            | ServerEvent::Ringing { .. }
            | ServerEvent::Answered { .. }
            | ServerEvent::TransportFailed { .. } => {}
            ServerEvent::Response { .. } => {
                // Client delegation never produces backend Responses events.
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedProviderEvent);
            }
            ServerEvent::Error { error, .. } => {
                if let (Some(inner), Some(outer)) =
                    (error.client_event_id.as_deref(), client_event_id.as_deref())
                    && inner != outer
                {
                    return Err(protocol_error());
                }
                let rejected = error
                    .client_event_id
                    .as_deref()
                    .or(client_event_id.as_deref());
                let pending = rejected
                    .and_then(|id| {
                        self.pending_appends
                            .iter()
                            .position(|pending| pending_event_id(pending.token) == id)
                    })
                    .and_then(|index| self.pending_appends.remove(index));
                let summary = summarize_unknown_provider_event("error", &raw);
                tracing::warn!(
                    provider_event_class = "error",
                    error_class = summary.error_class,
                    top_level_field_count = summary.top_level_field_count,
                    normalized_json_bytes = summary.normalized_json_bytes,
                    message_bytes = summary.message_bytes,
                    "public Live reported a provider error on the sideband"
                );
                // Closing rejects in-flight context injections. Preserve their
                // exact failed delivery, but keep draining the final transcript
                // and session.closed. An unrelated error remains fatal.
                if self.close_requested
                    && let Some(pending) = pending
                {
                    self.queued_observations.push_back(match pending.lane {
                        PendingAppendLane::Session => {
                            GptLiveBrokerObservation::SessionContextAppendRejected {
                                token: pending.token,
                            }
                        }
                        PendingAppendLane::Delegation => {
                            GptLiveBrokerObservation::DelegationContextAppendRejected {
                                token: pending.token,
                            }
                        }
                    });
                    return Ok(());
                }
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedProviderEvent);
            }
            ServerEvent::Unknown => {
                let kind = raw
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let summary = summarize_unknown_provider_event(kind, &raw);
                tracing::warn!(
                    provider_event_class = "unknown",
                    event_kind_sha256 = %summary.event_kind_sha256,
                    error_class = summary.error_class,
                    top_level_field_count = summary.top_level_field_count,
                    normalized_json_bytes = summary.normalized_json_bytes,
                    message_bytes = summary.message_bytes,
                    "public Live received an unsupported sideband event"
                );
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedProviderEvent);
            }
        }
        Ok(())
    }

    /// Resolve which pending append an acknowledgement settles. The echoed
    /// `client_event_id` is authoritative; without it only a single pending
    /// append is unambiguous.
    fn take_acknowledged_append(
        &mut self,
        client_event_id: Option<&str>,
    ) -> Result<PendingAppend, GptLiveBrokerError> {
        let index = match client_event_id {
            Some(id) => self
                .pending_appends
                .iter()
                .position(|pending| pending_event_id(pending.token) == id),
            None if self.pending_appends.len() == 1 => Some(0),
            None => None,
        };
        index
            .and_then(|index| self.pending_appends.remove(index))
            .ok_or_else(protocol_error)
    }

    fn record_transcript_delta(&mut self, role: GptLiveTurnRole, start_ms: f64, delta: String) {
        let turn = self.ensure_open_turn(role);
        self.next_transcript_item = self.next_transcript_item.saturating_add(1);
        let item = GptLiveTranscriptItemRef(format!(
            "{}:{}",
            match role {
                GptLiveTurnRole::User => "input",
                GptLiveTurnRole::Assistant | GptLiveTurnRole::Unknown => "output",
            },
            self.next_transcript_item
        ));
        self.queued_observations.push_back(match role {
            GptLiveTurnRole::User => GptLiveBrokerObservation::UserTranscriptFragment {
                item,
                text: delta.clone(),
            },
            GptLiveTurnRole::Assistant | GptLiveTurnRole::Unknown => {
                GptLiveBrokerObservation::AssistantTranscriptFragment {
                    item,
                    text: delta.clone(),
                }
            }
        });
        self.queued_observations
            .push_back(GptLiveBrokerObservation::TurnSnapshotDelta {
                turn,
                delta: delta.clone(),
            });
        if let Some(open) = self.open_turn.as_mut() {
            open.segments.push(TranscriptSegment {
                start_ms,
                text: delta,
            });
        }
    }

    /// Return the open turn for `role`, finishing a different-role turn and
    /// starting a new one when the speaker changes.
    fn ensure_open_turn(&mut self, role: GptLiveTurnRole) -> GptLiveTurnRef {
        if let Some(open) = self.open_turn.as_ref()
            && open.role == role
        {
            return GptLiveTurnRef(open.provider_ref.clone());
        }
        self.finish_open_turn();
        self.start_turn(role)
    }

    fn start_turn(&mut self, role: GptLiveTurnRole) -> GptLiveTurnRef {
        let turn = self.mint_turn(role);
        self.open_turn = Some(OpenTurn {
            provider_ref: turn.0.clone(),
            role,
            segments: Vec::new(),
        });
        turn
    }

    /// Announce a fresh synthesized turn without making it the open turn.
    fn mint_turn(&mut self, role: GptLiveTurnRole) -> GptLiveTurnRef {
        self.next_turn = self.next_turn.saturating_add(1);
        let turn = GptLiveTurnRef(format!("public-live-turn:{}", self.next_turn));
        self.queued_observations
            .push_back(GptLiveBrokerObservation::TurnStarted {
                turn: turn.clone(),
                role,
            });
        turn
    }

    fn finish_open_turn(&mut self) {
        let Some(open) = self.open_turn.take() else {
            return;
        };
        let transcript = join_segments(&open.segments);
        if open.role == GptLiveTurnRole::User {
            self.last_user_turn = Some(FinishedUserTurn {
                transcript: transcript.clone(),
            });
        }
        self.queued_observations
            .push_back(GptLiveBrokerObservation::TurnFinished {
                turn: GptLiveTurnRef(open.provider_ref),
                role: open.role,
                transcript,
            });
    }

    fn record_delegation(
        &mut self,
        delegation: Delegation,
        offset_ms: f64,
    ) -> Result<(), GptLiveBrokerError> {
        if delegation.item_type != DelegationType::Delegation
            || delegation.id.trim().is_empty()
            || self.seen_delegation_ids.contains(&delegation.id)
            || self.seen_delegation_ids.len() >= Self::MAX_DELEGATION_IDENTITIES
        {
            return Err(protocol_error());
        }
        self.seen_delegation_ids.insert(delegation.id.clone());
        let reference = GptLiveDelegationRef(delegation.id);
        if delegation.target != DelegationTarget::Client {
            self.queued_observations.push_back(
                GptLiveBrokerObservation::DelegationActionableInputUnsupported {
                    delegation: reference,
                },
            );
            return Ok(());
        }
        // Join the delegation to the open user turn, which it terminates. The
        // joined request is exactly the transcript prefix observed before the
        // delegation offset; speech that started at or after the offset is
        // not part of this request and continues as a fresh user turn. A
        // delegation arriving after the user stopped speaking (or while the
        // assistant speaks) re-presents the most recent frozen user transcript
        // under a fresh detached turn so the facade's start/finish pairing
        // stays exact and any open assistant turn continues undisturbed.
        let mut continued_segments = Vec::new();
        let (turn, transcript) = match self.open_turn.take() {
            Some(open) if open.role == GptLiveTurnRole::User => {
                let (before, after): (Vec<_>, Vec<_>) = open
                    .segments
                    .into_iter()
                    .partition(|segment| segment.start_ms < offset_ms);
                let transcript = join_segments(&before);
                self.last_user_turn = Some(FinishedUserTurn {
                    transcript: transcript.clone(),
                });
                continued_segments = after;
                (GptLiveTurnRef(open.provider_ref), transcript)
            }
            other => {
                self.open_turn = other;
                let Some(last) = self.last_user_turn.as_ref() else {
                    // No user input exists to become the task.
                    self.queued_observations.push_back(
                        GptLiveBrokerObservation::DelegationActionableInputUnsupported {
                            delegation: reference,
                        },
                    );
                    return Ok(());
                };
                let transcript = last.transcript.clone();
                (self.mint_turn(GptLiveTurnRole::User), transcript)
            }
        };
        self.queued_observations
            .push_back(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation: reference,
                target: GptLiveDelegationTarget::Client,
                turn,
                transcript,
            });
        if !continued_segments.is_empty() {
            // Speech that started at or after the offset is a new user turn.
            self.start_turn(GptLiveTurnRole::User);
            if let Some(open) = self.open_turn.as_mut() {
                open.segments = continued_segments;
            }
        }
        Ok(())
    }
}

fn pending_event_id(token: GptLiveAppendToken) -> String {
    format!("meerkat-append-{}", token.0)
}

fn map_live_error(error: LiveError) -> GptLiveBrokerError {
    let class = match error {
        LiveError::Invalid(reason) => {
            // Protocol-client validation, for example a startup history seed
            // over the provider's 128-message / 8,192-token limit. The class
            // alone is indistinguishable from a malformed provider event; the
            // reason names no credential or dialogue content.
            tracing::warn!(
                %reason,
                "public Live request rejected by protocol validation before it reached the provider"
            );
            GptLiveBrokerTerminalClass::Protocol
        }
        LiveError::Json(_)
        | LiveError::MalformedEvent { .. }
        | LiveError::Provider(_)
        | LiveError::SessionIdentityMismatch(_)
        | LiveError::ContinuityLost => GptLiveBrokerTerminalClass::Protocol,
        LiveError::Http {
            status, request_id, ..
        } => {
            // Operators otherwise see only a sanitized transport class; the
            // status and request id carry no credential or content material
            // and distinguish entitlement (403) from availability failures.
            tracing::warn!(
                status,
                request_id = request_id.as_deref().unwrap_or("<none>"),
                forbidden_hint = status == 403,
                "public Live HTTP request was rejected by the provider (403: unknown voice name or organization without Live access)"
            );
            GptLiveBrokerTerminalClass::Http
        }
        LiveError::Transport(_)
        | LiveError::Timeout
        | LiveError::AmbiguousWrite
        | LiveError::UnconfirmedClose => GptLiveBrokerTerminalClass::WebSocket,
        LiveError::Closed => GptLiveBrokerTerminalClass::Closed,
    };
    GptLiveBrokerError::Transport { class }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use meerkat_core::{
        AuthMetadata, Config, ModelRegistry, Provider, SessionLlmIdentity,
        connection::BackendProfile,
    };
    use meerkat_llm_core::provider_runtime::{ResolvedConnection, StaticLease};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn realtime_target(model: &str, backend_kind: OpenAiBackendKind) -> ResolvedRealtimeTarget {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("canonical model registry");
        let identity = SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let witness = registry
            .profile_witness_for_provider(Provider::OpenAI, &identity.model)
            .expect("registry witness");
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            backend_profile: Arc::new(BackendProfile {
                id: "test-backend".to_string(),
                provider: Provider::OpenAI,
                backend_kind: backend_kind.as_str().to_string(),
                base_url: None,
                options: Value::Null,
                server: None,
            }),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &meerkat_core::AuthBindingRef {
                    realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
                    binding: meerkat_core::BindingId::parse("openai").expect("valid binding"),
                    profile: None,
                    origin: meerkat_core::BindingOrigin::Configured,
                },
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "credential-secret".to_string(),
                AuthMetadata::default(),
                None,
                "openai:test",
            )),
        };
        ResolvedRealtimeTarget::new(identity, witness, connection).expect("matching target")
    }

    fn frame(value: Value) -> ServerFrame {
        oai_rt_rs::live::Codec::default()
            .decode_server(&value.to_string())
            .expect("fixture event decodes")
    }

    fn drain(state: &mut SessionState) -> Vec<GptLiveBrokerObservation> {
        state.queued_observations.drain(..).collect()
    }

    fn input_delta(text: &str) -> Value {
        input_delta_at(text, 0.0)
    }

    fn input_delta_at(text: &str, start_ms: f64) -> Value {
        json!({"type":"session.input_transcript.delta","event_id":"i","delta":text,"start_ms":start_ms,"end_ms":start_ms + 1.0})
    }

    fn ack(client_event_id: Option<&str>) -> Value {
        let mut value = json!({"type":"session.commentary.appended","event_id":"a","start_ms":1.0,"end_ms":1.0});
        if let Some(id) = client_event_id {
            value["client_event_id"] = json!(id);
        }
        value
    }

    fn output_delta(text: &str) -> Value {
        json!({"type":"session.output_transcript.delta","event_id":"o","delta":text,"start_ms":1.0,"end_ms":2.0})
    }

    fn delegation_created(id: &str, target: &str) -> Value {
        json!({"type":"session.delegation.created","event_id":"d","offset_ms":1.5,
            "delegation":{"type":"delegation","id":id,"target":target}})
    }

    fn assert_protocol_error(error: GptLiveBrokerError) {
        assert!(matches!(
            error,
            GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol
            }
        ));
    }

    #[test]
    fn factory_admits_only_released_gpt_live_rows_on_the_openai_api_backend() {
        assert!(matches!(
            PublicLiveBrokerFactory::try_from_target(realtime_target(
                "gpt-realtime-2",
                OpenAiBackendKind::OpenAiApi
            )),
            Err(ProviderClientError::MissingFeature(
                "openai-live-model-family"
            ))
        ));
        assert!(matches!(
            PublicLiveBrokerFactory::try_from_target(realtime_target(
                "gpt-live-1-codex",
                OpenAiBackendKind::OpenAiApi
            )),
            Err(ProviderClientError::MissingFeature(
                "openai-live-released-model"
            ))
        ));
        assert!(matches!(
            PublicLiveBrokerFactory::try_from_target(realtime_target(
                "gpt-live-1",
                OpenAiBackendKind::ChatGptBackend
            )),
            Err(ProviderClientError::MissingFeature(
                "openai-live-openai-api-backend"
            ))
        ));
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .expect("released public row admits");
        let rendered = format!("{factory:?}");
        assert!(!rendered.contains("credential-secret"));
        assert!(!rendered.contains("gpt-live-1"));
    }

    #[test]
    fn session_config_selects_client_delegation_voice_and_instructions_only() {
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .expect("admitted factory");
        let config = PublicLiveOpenConfig::new("v=0\r\nOFFER", "marin")
            .expect("valid config")
            .with_instructions("Keep answers short.");
        let session = factory.session_config(&config);
        assert_eq!(session.model, "gpt-live-1");
        let audio = session.audio.expect("voice selection");
        assert!(audio.format.is_none(), "WebRTC negotiates media");
        assert_eq!(
            audio.output.and_then(|output| output.voice),
            Some(Voice::Named("marin".to_string()))
        );
        assert_eq!(session.delegation, Field::Value(DelegationConfig::Client));
        assert_eq!(
            session.instructions,
            Field::Value("Keep answers short.".to_string())
        );
        assert!(session.client.is_none() && session.input.is_none() && session.store.is_none());
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("OFFER") && !rendered.contains("marin"));
        assert!(!rendered.contains("Keep answers short"));
    }

    #[test]
    fn startup_history_preserves_dialogue_roles_without_executor_authority() {
        use meerkat_core::types::{
            AssistantBlock, BlockAssistantMessage, StopReason, SystemMessage, ToolResult,
            UserMessage,
        };

        let history = vec![
            Message::System(SystemMessage::new("private executor instructions")),
            Message::User(UserMessage::text("Remember  the table.\n")),
            Message::tool_results(vec![ToolResult::new(
                "private-call".to_string(),
                "private tool output".to_string(),
                false,
            )]),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "Booked for two.".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )),
        ];
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&history);
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let encoded = serde_json::to_value(factory.session_config(&config)).unwrap();
        assert_eq!(
            encoded["input"],
            json!([
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "Remember  the table.\n"}
                ]},
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "Booked for two."}
                ]}
            ])
        );
        assert!(!encoded.to_string().contains("private"));
        assert!(!format!("{config:?}").contains("Remember"));
    }

    #[test]
    fn startup_history_preserves_unmeasured_assistant_provenance_when_flattened() {
        let history = [Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage::new(
                vec![meerkat_core::AssistantBlock::Transcript {
                    text: "Observed voice dialogue.".into(),
                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            ),
        )];
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&history);
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let encoded = serde_json::to_value(factory.session_config(&config)).unwrap();
        assert_eq!(encoded["input"][0]["role"], "assistant");
        let text = encoded["input"][0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Observed voice dialogue."));
        assert!(text.contains("UNMEASURED"));
        assert!(text.contains("Not proof"));
        assert!(encoded["input"][0].get("status").is_none());
    }

    #[test]
    fn startup_summary_is_factual_input_not_behavior_or_a_canonical_replay() {
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Speak briefly.")
            .with_context_summary("The agent is comparing two tables.");
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let encoded = serde_json::to_value(factory.session_config(&config)).unwrap();
        assert_eq!(encoded["instructions"], "Speak briefly.");
        assert_eq!(encoded["input"].as_array().unwrap().len(), 1);
        assert_eq!(encoded["input"][0]["role"], "user");
        let text = encoded["input"][0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("context data, not a new user request"));
        assert!(text.ends_with("The agent is comparing two tables."));
        assert!(!format!("{config:?}").contains("two tables"));
    }

    #[test]
    fn startup_history_does_not_silently_trim_over_limit_dialogue() {
        let messages = (0..129)
            .map(|index| Message::User(meerkat_core::types::UserMessage::text(index.to_string())))
            .collect::<Vec<_>>();
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let allowed = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&messages[..128]);
        assert!(factory.session_config(&allowed).validate().is_ok());
        let oversize = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&messages);
        assert!(factory.session_config(&oversize).validate().is_err());
        assert_eq!(oversize.input.len(), 129);
    }

    #[test]
    fn open_config_rejects_blank_mechanical_inputs() {
        assert!(matches!(
            PublicLiveOpenConfig::new("  ", "marin"),
            Err(GptLiveBrokerError::MissingOfferSdp)
        ));
        assert!(matches!(
            PublicLiveOpenConfig::new("v=0", " "),
            Err(GptLiveBrokerError::MissingVoice)
        ));
    }

    #[test]
    fn transcript_role_alternation_synthesizes_turns() {
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("hello "))).unwrap();
        state.apply_frame(frame(input_delta("there"))).unwrap();
        state.apply_frame(frame(output_delta("hi"))).unwrap();
        let observations = drain(&mut state);
        let kinds: Vec<_> = observations.iter().map(|o| format!("{o:?}")).collect();
        assert!(matches!(
            &observations[0],
            GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::User,
                ..
            }
        ));
        assert!(matches!(
            &observations[1],
            GptLiveBrokerObservation::UserTranscriptFragment { text, .. } if text == "hello "
        ));
        assert!(matches!(
            &observations[2],
            GptLiveBrokerObservation::TurnSnapshotDelta { delta, .. } if delta == "hello "
        ));
        assert!(matches!(
            &observations[3],
            GptLiveBrokerObservation::UserTranscriptFragment { text, .. } if text == "there"
        ));
        let GptLiveBrokerObservation::TurnFinished {
            turn: finished_user,
            role: GptLiveTurnRole::User,
            transcript,
        } = &observations[5]
        else {
            panic!("user turn must finish when the assistant starts: {kinds:?}");
        };
        assert_eq!(transcript, "hello there");
        let GptLiveBrokerObservation::TurnStarted {
            turn: started_user,
            role: GptLiveTurnRole::User,
        } = &observations[0]
        else {
            unreachable!()
        };
        assert_eq!(started_user, finished_user);
        assert!(matches!(
            &observations[6],
            GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::Assistant,
                ..
            }
        ));
        assert!(matches!(
            &observations[7],
            GptLiveBrokerObservation::AssistantTranscriptFragment { text, .. } if text == "hi"
        ));
        assert_eq!(observations.len(), 9);

        // Stream end flushes the open assistant turn with its full transcript.
        state.apply_frame(frame(output_delta(" there"))).unwrap();
        state.finish_open_turn();
        let tail = drain(&mut state);
        assert!(matches!(
            tail.last(),
            Some(GptLiveBrokerObservation::TurnFinished {
                role: GptLiveTurnRole::Assistant,
                transcript,
                ..
            }) if transcript == "hi there"
        ));
        assert!(
            !format!("{tail:?}").contains("hi there"),
            "Debug must redact text"
        );
    }

    #[test]
    fn client_delegation_terminates_the_open_user_turn_as_an_exact_join() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        let started = drain(&mut state);
        let GptLiveBrokerObservation::TurnStarted {
            turn: user_turn, ..
        } = &started[0]
        else {
            panic!("user turn start");
        };
        state
            .apply_frame(frame(delegation_created("dlg_1", "client")))
            .unwrap();
        let joined = drain(&mut state);
        assert_eq!(joined.len(), 1, "the join is the sole terminal observation");
        assert!(matches!(
            &joined[0],
            GptLiveBrokerObservation::ClientDelegationFinal { delegation, target: GptLiveDelegationTarget::Client, turn, transcript }
                if delegation.__opaque_provider_id() == "dlg_1" && turn == user_turn && transcript == "book a table"
        ));
        // The assistant reply starts a new turn without a second user finish.
        state.apply_frame(frame(output_delta("sure"))).unwrap();
        let next = drain(&mut state);
        assert!(matches!(
            &next[0],
            GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::Assistant,
                ..
            }
        ));
    }

    #[test]
    fn late_delegation_reuses_the_last_user_transcript_under_a_detached_turn() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("what time is it")))
            .unwrap();
        state
            .apply_frame(frame(output_delta("let me check")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(delegation_created("dlg_late", "client")))
            .unwrap();
        let observations = drain(&mut state);
        assert_eq!(observations.len(), 2);
        let GptLiveBrokerObservation::TurnStarted {
            turn: minted,
            role: GptLiveTurnRole::User,
        } = &observations[0]
        else {
            panic!("detached user turn must be announced first: {observations:?}");
        };
        assert!(matches!(
            &observations[1],
            GptLiveBrokerObservation::ClientDelegationFinal { turn, transcript, .. }
                if turn == minted && transcript == "what time is it"
        ));
        // The assistant turn stayed open and continues under its own ref.
        state.apply_frame(frame(output_delta(" now"))).unwrap();
        let cont = drain(&mut state);
        assert!(matches!(
            &cont[0],
            GptLiveBrokerObservation::AssistantTranscriptFragment { text, .. } if text == " now"
        ));
        assert!(
            !cont
                .iter()
                .any(|o| matches!(o, GptLiveBrokerObservation::TurnStarted { .. }))
        );
    }

    #[test]
    fn delegation_without_user_input_is_unsupported_not_a_join() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(delegation_created("dlg_0", "client")))
            .unwrap();
        let observations = drain(&mut state);
        assert!(matches!(
            observations.as_slice(),
            [GptLiveBrokerObservation::DelegationActionableInputUnsupported { delegation }]
                if delegation.__opaque_provider_id() == "dlg_0"
        ));
    }

    #[test]
    fn delegation_identity_and_target_fail_closed() {
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("x"))).unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_r", "responses")))
            .unwrap();
        assert!(matches!(
            drain(&mut state).last(),
            Some(GptLiveBrokerObservation::DelegationActionableInputUnsupported { .. })
        ));
        state
            .apply_frame(frame(delegation_created("dlg_dup", "client")))
            .unwrap();
        drain(&mut state);
        assert_protocol_error(
            state
                .apply_frame(frame(delegation_created("dlg_dup", "client")))
                .expect_err("duplicate delegation identity"),
        );
        assert_protocol_error(
            state
                .apply_frame(frame(delegation_created("  ", "client")))
                .expect_err("blank delegation identity"),
        );
    }

    #[test]
    fn append_acknowledgements_correlate_by_client_event_id_not_position() {
        let mut state = SessionState::default();
        let session_token = state.reserve_append(PendingAppendLane::Session).unwrap();
        let delegation_token = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        assert_ne!(session_token, delegation_token);
        // Two appends pending: an acknowledgement without an id is ambiguous.
        assert_protocol_error(
            state
                .apply_frame(frame(ack(None)))
                .expect_err("uncorrelated acknowledgement with two pending appends"),
        );
        // Out-of-order acknowledgement resolves by echoed id, not by position.
        state
            .apply_frame(frame(ack(Some(&pending_event_id(delegation_token)))))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token }]
                if *token == delegation_token
        ));
        // A single pending append accepts an id-less acknowledgement.
        state.apply_frame(frame(ack(None))).unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::SessionContextAppendAcknowledged { token }]
                if *token == session_token
        ));
        assert_protocol_error(
            state
                .apply_frame(frame(ack(None)))
                .expect_err("acknowledgement without a pending append"),
        );
        let stray = state.reserve_append(PendingAppendLane::Session).unwrap();
        assert_protocol_error(
            state
                .apply_frame(frame(ack(Some("meerkat-append-999"))))
                .expect_err("acknowledgement for an unknown append id"),
        );
        assert_eq!(state.pending_appends.len(), 1);
        let _ = stray;
        state.append_delivery_ambiguous = true;
        assert!(matches!(
            state.reserve_append(PendingAppendLane::Session),
            Err(GptLiveBrokerError::AppendInFlight)
        ));
    }

    #[test]
    fn delegation_freezes_only_the_transcript_prefix_before_its_offset() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta_at("book a table ", 0.0)))
            .unwrap();
        state
            .apply_frame(frame(input_delta_at("for two", 1.0)))
            .unwrap();
        state
            .apply_frame(frame(input_delta_at(" and also", 9.0)))
            .unwrap();
        let started = drain(&mut state);
        let GptLiveBrokerObservation::TurnStarted {
            turn: first_turn, ..
        } = &started[0]
        else {
            panic!("user turn start");
        };
        state
            .apply_frame(frame(
                json!({"type":"session.delegation.created","event_id":"d","offset_ms":4.2,
                "delegation":{"type":"delegation","id":"dlg_prefix","target":"client"}}),
            ))
            .unwrap();
        let joined = drain(&mut state);
        assert!(matches!(
            &joined[0],
            GptLiveBrokerObservation::ClientDelegationFinal { turn, transcript, .. }
                if turn == first_turn && transcript == "book a table for two"
        ));
        // Speech that started after the offset continues as a fresh user turn.
        assert!(matches!(
            &joined[1],
            GptLiveBrokerObservation::TurnStarted { role: GptLiveTurnRole::User, turn } if turn != first_turn
        ));
        assert_eq!(joined.len(), 2);
        state.apply_frame(frame(output_delta("sure"))).unwrap();
        let next = drain(&mut state);
        assert!(matches!(
            &next[0],
            GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::User, transcript, .. }
                if transcript == " and also"
        ));
    }

    #[test]
    fn rejected_append_releases_its_reservation_and_surfaces_unsupported() {
        let mut state = SessionState::default();
        let token = state.reserve_append(PendingAppendLane::Session).unwrap();
        state
            .apply_frame(frame(json!({"type":"error","event_id":"e","error":{
                "type":"invalid_request_error","code":"invalid_value","message":"Invalid value FIXTURE_SECRET",
                "param":"content","client_event_id":pending_event_id(token)}})))
            .unwrap();
        assert!(state.pending_appends.is_empty());
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::UnsupportedProviderEvent]
        ));
        state
            .apply_frame(frame(
                json!({"type":"session.future.event","event_id":"f","payload":"FIXTURE_PAYLOAD"}),
            ))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::UnsupportedProviderEvent]
        ));
        state
            .apply_frame(frame(json!({"type":"response.event","event_id":"r","delegation_id":"dlg","event":{"type":"response.created"}})))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::UnsupportedProviderEvent]
        ));
    }

    #[test]
    fn close_rejections_preserve_exact_append_failures_and_terminal_tail() {
        let mut state = SessionState::default();
        let session = state.reserve_append(PendingAppendLane::Session).unwrap();
        let delegation = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        state
            .apply_frame(frame(output_delta("final reply")))
            .unwrap();
        drain(&mut state);
        state.close_requested = true;
        for token in [delegation, session] {
            state
                .apply_frame(frame(json!({"type":"error","event_id":"e","error":{
                    "type":"invalid_request_error","code":null,"message":"PRIVATE_ERROR_PAYLOAD",
                    "client_event_id":pending_event_id(token)
                }})))
                .unwrap();
        }
        state
            .apply_frame(frame(
                json!({"type":"session.closed","event_id":"c","session":{
            "id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0
        },"reason":"close_requested","usage":{"seconds":1.0}}),
            ))
            .unwrap();
        let observations = drain(&mut state);
        assert!(matches!(
            observations.as_slice(),
            [
                GptLiveBrokerObservation::DelegationContextAppendRejected { token: rejected_delegation },
                GptLiveBrokerObservation::SessionContextAppendRejected { token: rejected_session },
                GptLiveBrokerObservation::TurnFinished { transcript, .. },
            ] if *rejected_delegation == delegation && *rejected_session == session
                && transcript == "final reply"
        ));
        assert!(state.pending_appends.is_empty());
        assert!(state.closed_observed);
        assert!(!format!("{observations:?}").contains("PRIVATE_ERROR_PAYLOAD"));
    }

    #[test]
    fn close_does_not_launder_unknown_or_conflicting_append_errors() {
        let mut state = SessionState::default();
        let pending = state.reserve_append(PendingAppendLane::Session).unwrap();
        state.close_requested = true;
        for reference in [None, Some("unrelated"), Some("meerkat-append-999")] {
            let mut event = json!({"type":"error","event_id":"e","error":{
                "type":"invalid_request_error","code":null,"message":"rejected"
            }});
            if let Some(reference) = reference {
                event["error"]["client_event_id"] = json!(reference);
            }
            state.apply_frame(frame(event)).unwrap();
            assert!(matches!(
                drain(&mut state).as_slice(),
                [GptLiveBrokerObservation::UnsupportedProviderEvent]
            ));
            assert_eq!(state.pending_appends.len(), 1);
        }
        assert_protocol_error(
            state
                .apply_frame(frame(json!({
                    "type":"error","event_id":"e","client_event_id":"unrelated","error":{
                        "type":"invalid_request_error","code":null,"message":"rejected",
                        "client_event_id":pending_event_id(pending)
                    }
                })))
                .unwrap_err(),
        );
        assert_eq!(state.pending_appends.len(), 1);
    }

    #[test]
    fn media_telemetry_and_readiness_frames_carry_no_observation() {
        let mut state = SessionState::default();
        for value in [
            json!({"type":"session.started","event_id":"s","session":{"id":"live_x","model":"gpt-live-1","status":"active","expires_at":1.0}}),
            json!({"type":"session.output_audio.delta","delta":"AAAA","start_ms":0.0,"end_ms":1.0}),
            json!({"type":"session.input_audio.append","audio":"AAAA"}),
            json!({"type":"session.usage.updated","event_id":"u","usage":{"seconds":1.0}}),
            json!({"type":"session.input_audio.muted","event_id":"m"}),
            json!({"type":"info","event_id":"i","code":"note","message":"FIXTURE"}),
        ] {
            state.apply_frame(frame(value)).unwrap();
        }
        assert!(state.queued_observations.is_empty());
    }

    #[test]
    fn live_errors_lower_to_sanitized_terminal_classes() {
        let class = |error: LiveError| match map_live_error(error) {
            GptLiveBrokerError::Transport { class } => class,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(class(LiveError::Closed), GptLiveBrokerTerminalClass::Closed);
        assert_eq!(
            class(LiveError::Timeout),
            GptLiveBrokerTerminalClass::WebSocket
        );
        assert_eq!(
            class(LiveError::Invalid("x".into())),
            GptLiveBrokerTerminalClass::Protocol
        );
        assert_eq!(
            class(LiveError::ContinuityLost),
            GptLiveBrokerTerminalClass::Protocol
        );
    }

    #[derive(Default)]
    struct Capture {
        create_body: Option<Value>,
        create_authorization: Option<String>,
        attach_authorization: Option<String>,
        client_events: Vec<Value>,
    }
    type SharedCapture = Arc<std::sync::Mutex<Capture>>;

    async fn create_session(
        State(capture): State<SharedCapture>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let mut capture = capture.lock().expect("capture lock");
        capture.create_body = serde_json::from_slice(&body).ok();
        capture.create_authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        (
            StatusCode::CREATED,
            [("content-type", "application/json")],
            json!({"session":{"id":"live_fixture"},"transport":{"type":"webrtc","sdp":"v=0\r\nPUBLIC_ANSWER_SDP"}})
                .to_string(),
        )
            .into_response()
    }

    async fn attach(
        State(capture): State<SharedCapture>,
        headers: HeaderMap,
        upgrade: WebSocketUpgrade,
    ) -> Response {
        capture.lock().expect("capture lock").attach_authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        upgrade.on_upgrade(move |socket| serve_sideband(socket, capture))
    }

    async fn send_json(socket: &mut WebSocket, value: Value) {
        socket
            .send(AxumMessage::Text(value.to_string().into()))
            .await
            .expect("fixture send");
    }

    async fn recv_json(socket: &mut WebSocket, capture: &SharedCapture) -> Value {
        loop {
            match socket.recv().await {
                Some(Ok(AxumMessage::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).expect("client event JSON");
                    capture
                        .lock()
                        .expect("capture lock")
                        .client_events
                        .push(value.clone());
                    return value;
                }
                Some(Ok(_)) => continue,
                other => panic!("sideband closed early: {other:?}"),
            }
        }
    }

    async fn serve_sideband(mut socket: WebSocket, capture: SharedCapture) {
        let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":12345.5});
        send_json(
            &mut socket,
            json!({"type":"session.started","event_id":"s","session":snapshot}),
        )
        .await;
        // Media reflection and a transcript race ahead of the seed acknowledgement.
        send_json(
            &mut socket,
            json!({"type":"session.input_audio.append","audio":"AAAA"}),
        )
        .await;
        send_json(&mut socket, input_delta("book me a table")).await;
        let seed = recv_json(&mut socket, &capture).await;
        assert_eq!(seed["type"], "session.commentary.append");
        assert!(seed["delegation_id"].is_null());
        send_json(&mut socket, json!({"type":"session.commentary.appended","event_id":"a1","client_event_id":seed["event_id"],"start_ms":1.0,"end_ms":1.0})).await;
        send_json(&mut socket, delegation_created("dlg_public", "client")).await;
        send_json(&mut socket, output_delta("one moment")).await;
        let release = recv_json(&mut socket, &capture).await;
        assert_eq!(release["type"], "session.commentary.append");
        assert_eq!(release["delegation_id"], "dlg_public");
        send_json(&mut socket, json!({"type":"session.commentary.appended","event_id":"a2","client_event_id":release["event_id"],"start_ms":2.0,"end_ms":2.0})).await;
        let close = recv_json(&mut socket, &capture).await;
        assert_eq!(close["type"], "session.close");
        send_json(&mut socket, json!({"type":"session.closed","event_id":"c","session":snapshot,"reason":"close_requested","usage":{"seconds":2.5}})).await;
        drop(socket);
    }

    async fn local_server() -> (String, SharedCapture, tokio::task::JoinHandle<()>) {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        (format!("http://{address}/v1/"), capture, server)
    }

    #[tokio::test]
    async fn broker_creates_attaches_seeds_and_lowers_public_events() {
        let (base_url, capture, server) = local_server().await;
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &base_url,
        )
        .expect("admitted factory");
        let config = PublicLiveOpenConfig::new("v=0\r\nOFFER_SDP", "marin")
            .expect("valid config")
            .with_instructions("Catalog guidance.");
        let bootstrap = factory.open(config).await.expect("public bootstrap");
        assert_eq!(bootstrap.answer_sdp(), "v=0\r\nPUBLIC_ANSWER_SDP");
        assert!(!format!("{bootstrap:?}").contains("PUBLIC_ANSWER_SDP"));
        let (_, session) = bootstrap.into_parts();
        {
            let capture = capture.lock().expect("capture lock");
            let body = capture.create_body.as_ref().expect("create body");
            assert_eq!(body["session"]["model"], "gpt-live-1");
            assert_eq!(body["session"]["delegation"]["type"], "client");
            assert_eq!(body["session"]["audio"]["output"]["voice"], "marin");
            assert!(body["session"]["audio"].get("format").is_none());
            assert_eq!(body["session"]["instructions"], "Catalog guidance.");
            assert_eq!(body["transport"]["type"], "webrtc");
            assert_eq!(body["transport"]["sdp"], "v=0\r\nOFFER_SDP");
            assert_eq!(
                capture.create_authorization.as_deref(),
                Some("Bearer credential-secret")
            );
            assert_eq!(
                capture.attach_authorization.as_deref(),
                Some("Bearer credential-secret")
            );
        }

        session
            .await_ready_and_seed_session_context(Some("{\"canonical_messages\":[]}".to_string()))
            .await
            .expect("seed acknowledged");
        // Observations that raced ahead of the acknowledgement are preserved in order.
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::User,
                ..
            })
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::UserTranscriptFragment { text, .. }) if text == "book me a table"
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnSnapshotDelta { .. })
        ));
        let delegation = match session.next_observation().await.unwrap() {
            Some(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation,
                target: GptLiveDelegationTarget::Client,
                transcript,
                ..
            }) if transcript == "book me a table" => delegation,
            other => panic!("expected joined delegation, got {other:?}"),
        };
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::Assistant,
                ..
            })
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::AssistantTranscriptFragment { text, .. }) if text == "one moment"
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnSnapshotDelta { .. })
        ));
        let token = session
            .append_delegation_context(&delegation, "Table booked for two.")
            .await
            .expect("delegation append");
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acked }) if acked == token
        ));
        session.close().await.expect("close requested");
        session
            .close()
            .await
            .expect("duplicate request is idempotent");
        // The terminal event flushes the assistant transcript exactly once.
        let mut finished = Vec::new();
        while let Some(observation) = session.next_observation().await.unwrap() {
            finished.push(observation);
        }
        session
            .close()
            .await
            .expect("confirmed close is idempotent");
        assert!(matches!(
            finished.as_slice(),
            [GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::Assistant, transcript, .. }]
                if transcript == "one moment"
        ));
        let events = capture.lock().expect("capture lock").client_events.clone();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["content"], "{\"canonical_messages\":[]}");
        assert_eq!(events[1]["content"], "Table booked for two.");
        assert!(
            events[..2]
                .iter()
                .all(|event| event["event_id"].is_string())
        );
        assert_eq!(events[2]["type"], "session.close");
        server.abort();
    }

    #[test]
    fn delegation_acknowledgement_does_not_finish_assistant_output() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_hold", "client")))
            .unwrap();
        state
            .apply_frame(frame(output_delta("one moment")))
            .unwrap();
        let original_turn = state.open_turn.as_ref().unwrap().provider_ref.clone();
        drain(&mut state);
        let token = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        state
            .apply_frame(frame(ack(Some(&pending_event_id(token)))))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acknowledged }]
                if *acknowledged == token
        ));
        state.apply_frame(frame(output_delta(", booked"))).unwrap();
        assert_eq!(
            state.open_turn.as_ref().unwrap().provider_ref,
            original_turn
        );
        assert_eq!(
            join_segments(&state.open_turn.as_ref().unwrap().segments),
            "one moment, booked"
        );
    }

    #[tokio::test]
    async fn long_pause_and_delayed_delegation_readout_preserve_output_identity() {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        async fn quiet_after_output(mut socket: WebSocket, capture: SharedCapture) {
            let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0});
            send_json(
                &mut socket,
                json!({"type":"session.started","event_id":"s","session":snapshot}),
            )
            .await;
            send_json(&mut socket, input_delta("book a table")).await;
            send_json(&mut socket, delegation_created("dlg_pause", "client")).await;
            send_json(&mut socket, output_delta("one moment")).await;
            send_json(&mut socket, json!({"type":"session.output_audio.delta","delta":"AAAA","start_ms":1.0,"end_ms":2.0})).await;
            let result = recv_json(&mut socket, &capture).await;
            send_json(&mut socket, ack(result["event_id"].as_str())).await;
            // Exceed both the old quiet timeout and its result-readout grace.
            tokio::time::sleep(std::time::Duration::from_millis(4500)).await;
            send_json(&mut socket, output_delta(", booked")).await;
            tokio::time::sleep(std::time::Duration::from_millis(1750)).await;
            send_json(&mut socket, output_delta(" for two")).await;
            let close = recv_json(&mut socket, &capture).await;
            assert_eq!(close["type"], "session.close");
            send_json(&mut socket, json!({"type":"session.closed","event_id":"c","session":snapshot,"reason":"close_requested","usage":{"seconds":6.5}})).await;
        }
        async fn attach_quiet(
            State(capture): State<SharedCapture>,
            upgrade: WebSocketUpgrade,
        ) -> Response {
            upgrade.on_upgrade(move |socket| quiet_after_output(socket, capture))
        }
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach_quiet))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &format!("http://{address}/v1/"),
        )
        .expect("admitted factory");
        let (_, session) = factory
            .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
            .await
            .expect("bootstrap")
            .into_parts();
        let delegation = loop {
            if let Some(GptLiveBrokerObservation::ClientDelegationFinal { delegation, .. }) =
                session.next_observation().await.unwrap()
            {
                break delegation;
            }
        };
        let mut observations = Vec::new();
        for _ in 0..3 {
            observations.push(session.next_observation().await.unwrap().unwrap());
        }
        let token = session
            .append_delegation_context(&delegation, "Booked.")
            .await
            .unwrap();
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acknowledged })
                if acknowledged == token
        ));
        for _ in 0..4 {
            let observation = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                session.next_observation(),
            )
            .await
            .expect("provider continuation arrives")
            .unwrap()
            .expect("stream stays open");
            observations.push(observation);
        }
        assert!(matches!(
            observations.as_slice(),
            [
                GptLiveBrokerObservation::TurnStarted { role: GptLiveTurnRole::Assistant, turn: started_turn },
                GptLiveBrokerObservation::AssistantTranscriptFragment { text: first, .. },
                GptLiveBrokerObservation::TurnSnapshotDelta { turn: first_turn, .. },
                GptLiveBrokerObservation::AssistantTranscriptFragment { text: second, .. },
                GptLiveBrokerObservation::TurnSnapshotDelta { turn: second_turn, .. },
                GptLiveBrokerObservation::AssistantTranscriptFragment { text: third, .. },
                GptLiveBrokerObservation::TurnSnapshotDelta { turn: third_turn, .. },
            ] if started_turn == first_turn && started_turn == second_turn && started_turn == third_turn
                && first == "one moment" && second == ", booked" && third == " for two"
        ));
        session.close().await.unwrap();
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::Assistant, transcript, .. })
                if transcript == "one moment, booked for two"
        ));
        assert!(session.next_observation().await.unwrap().is_none());
        server.abort();
    }

    #[tokio::test]
    async fn transport_eof_before_or_during_close_without_session_closed_never_finishes_output() {
        for during_close in [false, true] {
            let attach_unconfirmed = move |upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    send_json(
                    &mut socket,
                    json!({"type":"session.started","event_id":"s","session":{
                        "id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0
                    }}),
                )
                .await;
                    send_json(&mut socket, output_delta("unfinished")).await;
                    if during_close {
                        let message = socket.recv().await.unwrap().unwrap();
                        let AxumMessage::Text(text) = message else {
                            panic!("expected close request");
                        };
                        let request: Value = serde_json::from_str(&text).unwrap();
                        assert_eq!(request["type"], "session.close");
                    }
                    socket.send(AxumMessage::Close(None)).await.unwrap();
                })
            };
            let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
            let app = Router::new()
                .route("/v1/live/sessions", post(create_session))
                .route(
                    "/v1/live/sessions/{session_id}/attach",
                    get(attach_unconfirmed),
                )
                .with_state(capture);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
                realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
                &format!("http://{address}/v1/"),
            )
            .unwrap();
            let (_, session) = factory
                .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
                .await
                .unwrap()
                .into_parts();
            for _ in 0..3 {
                let observation = session.next_observation().await.unwrap().unwrap();
                assert!(!matches!(
                    observation,
                    GptLiveBrokerObservation::TurnFinished { .. }
                ));
            }
            if during_close {
                session
                    .close()
                    .await
                    .expect("request close before bare transport EOF");
            }
            assert!(matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    session.next_observation()
                )
                .await
                .expect("closed socket is observed"),
                Err(GptLiveBrokerError::Transport {
                    class: GptLiveBrokerTerminalClass::WebSocket
                })
            ));
            let state = session.state.lock().await;
            assert!(!state.closed_observed);
            assert_eq!(state.close_requested, during_close);
            assert_eq!(
                join_segments(&state.open_turn.as_ref().unwrap().segments),
                "unfinished"
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn seed_failure_closes_without_retrying() {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        async fn reject_seed(mut socket: WebSocket, capture: SharedCapture) {
            let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0});
            send_json(
                &mut socket,
                json!({"type":"session.started","event_id":"s","session":snapshot}),
            )
            .await;
            let _seed = recv_json(&mut socket, &capture).await;
            send_json(
                &mut socket,
                json!({"type":"session.future.event","event_id":"x"}),
            )
            .await;
            let close = recv_json(&mut socket, &capture).await;
            assert_eq!(close["type"], "session.close");
        }
        async fn attach_reject(
            State(capture): State<SharedCapture>,
            upgrade: WebSocketUpgrade,
        ) -> Response {
            upgrade.on_upgrade(move |socket| reject_seed(socket, capture))
        }
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach_reject))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &format!("http://{address}/v1/"),
        )
        .expect("admitted factory");
        let (_, session) = factory
            .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
            .await
            .expect("bootstrap")
            .into_parts();
        assert_protocol_error(
            session
                .await_ready_and_seed_session_context(Some("seed".to_string()))
                .await
                .expect_err("unsupported event during seed fails closed"),
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if capture
                    .lock()
                    .unwrap()
                    .client_events
                    .iter()
                    .any(|event| event["type"] == "session.close")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("close requested after seed failure");
        assert_eq!(
            capture
                .lock()
                .unwrap()
                .client_events
                .iter()
                .filter(|event| event["type"] == "session.commentary.append")
                .count(),
            1,
            "the seed append is never retried"
        );
        server.abort();
    }
}
