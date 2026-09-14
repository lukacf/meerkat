//! Public continuous observations never enter the legacy turn/tool routes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode,
    LiveAdapterObservation, LiveAdapterStatus, LiveChannelCapabilities, LiveDegradationReason,
    LiveInputChunk,
};
use meerkat_core::live_execution::backend::{
    LiveBackendOwnership, LiveProviderDiagnostic, LiveProviderDiagnosticCategory,
};
use meerkat_core::live_execution::frontend::{ContinuousLiveFrontendPolicy, LiveAudioIngress};
use meerkat_core::live_execution::observation::{
    ContinuousLiveObservation, LiveUsageDispute, LiveUsageSnapshot,
};
use meerkat_core::live_execution::request::LiveProviderReference;
use meerkat_core::live_observation::{
    LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
};
use meerkat_core::model_profile::ModelInteractionKind;
use meerkat_llm_core::live_adapter_factory::{
    ContinuousLiveOpenConfig, LiveAdapterFactory, LiveAdapterOpenConfig, LiveAdapterOpenError,
    LiveWebrtcPreparation,
};
use meerkat_llm_core::provider_runtime::{
    ProviderClientError, ResolvedLiveExecution, ResolvedLiveTarget,
};
use oai_rt_rs::live::{
    AudioFormat, AudioSource, ClientEvent, Command, DelegationTarget, LiveConnection, LiveReceiver,
    LiveSender, ServerEvent, ServerFrame, SessionPhase,
};
use tokio::sync::{Mutex, mpsc};

use super::config::PublicLiveVoiceSettings;
use super::session::{OpenAiPublicLiveSessionFactory, PublicLiveSessionError};
use super::voice_usage::observe_voice_usage;

pub struct OpenAiContinuousAdapterFactory {
    target: ResolvedLiveTarget,
}

impl OpenAiContinuousAdapterFactory {
    pub fn new(target: ResolvedLiveTarget) -> Result<Self, ProviderClientError> {
        if !matches!(
            target.execution(),
            ResolvedLiveExecution::ClientContext { .. }
        ) {
            return Err(ProviderClientError::MissingFeature(
                "openai-continuous-function-observations",
            ));
        }
        Ok(Self { target })
    }
}

#[async_trait::async_trait]
impl LiveAdapterFactory for OpenAiContinuousAdapterFactory {
    fn interaction_kind(&self) -> ModelInteractionKind {
        ModelInteractionKind::ContinuousLive
    }

    async fn open_adapter(
        &self,
        config: &LiveAdapterOpenConfig,
    ) -> Result<Arc<dyn LiveAdapter>, LiveAdapterOpenError> {
        let LiveAdapterOpenConfig::Continuous(config) = config else {
            return Err(LiveAdapterOpenError::InteractionMismatch {
                expected: self.interaction_kind(),
                actual: config.interaction_kind(),
            });
        };
        let factory = OpenAiPublicLiveSessionFactory::new(
            &self.target,
            PublicLiveVoiceSettings {
                voice: config.voice.as_deref(),
                instructions: config.instructions.as_deref(),
            },
            config.limits,
        )
        .map_err(open_error)?;
        let connection = factory.open_websocket().await.map_err(open_error)?;
        Ok(adapter(
            connection,
            config,
            None,
            LiveAudioIngress::PcmWebSocket {
                sample_rate_hz: 24_000,
                channels: 1,
            },
        )?)
    }

    async fn prepare_webrtc(
        &self,
        config: &LiveAdapterOpenConfig,
        offer_sdp: &str,
    ) -> Result<Box<dyn LiveWebrtcPreparation>, LiveAdapterOpenError> {
        let LiveAdapterOpenConfig::Continuous(config) = config else {
            return Err(LiveAdapterOpenError::InteractionMismatch {
                expected: self.interaction_kind(),
                actual: config.interaction_kind(),
            });
        };
        if offer_sdp.len() > config.limits.max_event_bytes {
            return Err(open_error(PublicLiveSessionError::RequestTooLarge).into());
        }
        let factory = OpenAiPublicLiveSessionFactory::new(
            &self.target,
            PublicLiveVoiceSettings {
                voice: config.voice.as_deref(),
                instructions: config.instructions.as_deref(),
            },
            config.limits,
        )
        .map_err(open_error)?;
        let created = factory
            .create_webrtc(offer_sdp.to_owned())
            .await
            .map_err(open_error)?;
        Ok(Box::new(OpenAiWebrtcPreparation {
            factory,
            config: config.clone(),
            provider_session: created.session.id,
            answer: created.transport.sdp().to_owned(),
            attached: None,
        }))
    }
}

struct OpenAiWebrtcPreparation {
    factory: OpenAiPublicLiveSessionFactory,
    config: ContinuousLiveOpenConfig,
    provider_session: String,
    answer: String,
    attached: Option<Arc<OpenAiContinuousAdapter>>,
}

#[async_trait::async_trait]
impl LiveWebrtcPreparation for OpenAiWebrtcPreparation {
    fn answer_sdp(&self) -> &str {
        &self.answer
    }
    async fn attach_adapter(&mut self) -> Result<Arc<dyn LiveAdapter>, LiveAdapterOpenError> {
        if let Some(adapter) = &self.attached {
            return Ok(adapter.clone());
        }
        let connection = self
            .factory
            .attach_sideband(&self.provider_session)
            .await
            .map_err(open_error)?;
        let adapter = adapter(
            connection,
            &self.config,
            Some(self.provider_session.clone()),
            LiveAudioIngress::WebRtcMediaTracks,
        )?;
        self.attached = Some(Arc::clone(&adapter));
        Ok(adapter)
    }
}

fn adapter(
    connection: LiveConnection,
    config: &ContinuousLiveOpenConfig,
    provider_session: Option<String>,
    audio_ingress: LiveAudioIngress,
) -> Result<Arc<OpenAiContinuousAdapter>, LiveAdapterError> {
    let (injected, injected_rx) = mpsc::channel(config.limits.event_capacity);
    let (sender, receiver) = connection.split();
    let started = Arc::new(AtomicBool::new(false));
    let identity_rejected = Arc::new(AtomicBool::new(false));
    let closed_confirmed = Arc::new(AtomicBool::new(false));
    Ok(Arc::new(OpenAiContinuousAdapter {
        sender,
        receiver: Mutex::new(ReceiverState {
            receiver,
            provider_session,
            injected: injected_rx,
            reported_end: false,
            started: Arc::clone(&started),
            identity_rejected: Arc::clone(&identity_rejected),
            closed_confirmed: Arc::clone(&closed_confirmed),
        }),
        injected,
        started,
        identity_rejected,
        closed_confirmed,
        frontend: ContinuousLiveFrontendPolicy::new(audio_ingress).map_err(|reason| {
            LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::ContinuousInputRejected { reason },
                message: reason.to_string(),
            }
        })?,
        close_timeout: config.limits.io_timeout,
    }))
}

struct OpenAiContinuousAdapter {
    sender: LiveSender,
    receiver: Mutex<ReceiverState>,
    injected: mpsc::Sender<LiveAdapterObservation>,
    frontend: ContinuousLiveFrontendPolicy,
    close_timeout: std::time::Duration,
    started: Arc<AtomicBool>,
    identity_rejected: Arc<AtomicBool>,
    closed_confirmed: Arc<AtomicBool>,
}

struct ReceiverState {
    receiver: LiveReceiver,
    provider_session: Option<String>,
    injected: mpsc::Receiver<LiveAdapterObservation>,
    reported_end: bool,
    started: Arc<AtomicBool>,
    identity_rejected: Arc<AtomicBool>,
    closed_confirmed: Arc<AtomicBool>,
}

fn identity_error() -> LiveAdapterError {
    LiveAdapterError::ProviderError {
        code: LiveAdapterErrorCode::ConnectionLost,
        message: "public Live session identity is unconfirmed or inconsistent".into(),
    }
}

impl ReceiverState {
    fn reject_identity(&self) -> LiveAdapterError {
        self.identity_rejected.store(true, Ordering::Release);
        self.started.store(false, Ordering::Release);
        identity_error()
    }
}

fn internal(event: ContinuousLiveObservation) -> LiveAdapterObservation {
    LiveAdapterObservation::Continuous {
        event,
        receive: None,
    }
}

fn diagnostic(
    category: LiveProviderDiagnosticCategory,
) -> Result<LiveAdapterObservation, LiveAdapterError> {
    LiveProviderDiagnostic::new(
        category,
        LiveBackendOwnership::Unowned {},
        std::num::NonZeroU64::MIN,
    )
    .map(|diagnostic| internal(ContinuousLiveObservation::Diagnostic(diagnostic)))
    .map_err(|error| LiveAdapterError::ProviderError {
        code: LiveAdapterErrorCode::InternalError,
        message: error.to_string(),
    })
}

fn open_error(error: PublicLiveSessionError) -> LiveAdapterError {
    let code = match &error {
        PublicLiveSessionError::MissingCredential
        | PublicLiveSessionError::UnsupportedAuthentication => {
            LiveAdapterErrorCode::AuthenticationFailed
        }
        PublicLiveSessionError::Transport(_) => LiveAdapterErrorCode::ConnectionFailed,
        PublicLiveSessionError::UnsupportedBackend
        | PublicLiveSessionError::InvalidApiRoot
        | PublicLiveSessionError::InvalidLimits
        | PublicLiveSessionError::RequestTooLarge
        | PublicLiveSessionError::Configuration(_) => LiveAdapterErrorCode::InternalError,
    };
    LiveAdapterError::ProviderError {
        code,
        message: error.to_string(),
    }
}

fn transport_error(error: oai_rt_rs::live::Error) -> LiveAdapterError {
    LiveAdapterError::TransportError {
        message: error.to_string(),
    }
}

fn transcript_event(
    direction: LiveTranscriptDirection,
    delta: String,
    start: f64,
    end: f64,
) -> ContinuousLiveObservation {
    match LiveTranscriptRange::new(start, end) {
        Ok(range) => ContinuousLiveObservation::Transcript(LiveTranscriptObservation::new(
            direction, range, delta,
        )),
        Err(error) => ContinuousLiveObservation::TranscriptRejected(error),
    }
}

#[async_trait::async_trait]
impl LiveAdapter for OpenAiContinuousAdapter {
    async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        if self.identity_rejected.load(Ordering::Acquire) {
            return Err(identity_error());
        }
        self.frontend
            .validate(&command)
            .map_err(|reason| LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::ContinuousInputRejected { reason },
                message: reason.to_string(),
            })?;
        if !self.started.load(Ordering::Acquire) {
            return Err(LiveAdapterError::NotReady {
                status: self.status(),
            });
        }
        match command {
            LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Audio { data, .. },
            } => self.sender.send_audio(&data).await.map_err(transport_error),
            LiveAdapterCommand::Close => self
                .sender
                .send(ClientEvent::new(Command::Close))
                .await
                .map_err(transport_error),
            _ => Err(LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::InternalError,
                message: "continuous input policy admitted an unrealized command".into(),
            }),
        }
    }

    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        let mut state = self.receiver.lock().await;
        loop {
            if state.reported_end {
                return Ok(None);
            }
            let ReceiverState {
                receiver, injected, ..
            } = &mut *state;
            let received = tokio::select! {
                Some(observation) = injected.recv() => return Ok(Some(observation)),
                frame = receiver.next_event() => frame,
            };
            match received {
                Ok(Some(frame)) => {
                    if let Some(observation) = project(&mut state, frame, self.sender.role())? {
                        return Ok(Some(observation));
                    }
                }
                Ok(None) if state.reported_end => return Ok(None),
                Ok(None)
                | Err(
                    oai_rt_rs::live::Error::Transport(_)
                    | oai_rt_rs::live::Error::Timeout
                    | oai_rt_rs::live::Error::AmbiguousWrite
                    | oai_rt_rs::live::Error::UnconfirmedClose
                    | oai_rt_rs::live::Error::ContinuityLost
                    | oai_rt_rs::live::Error::Closed
                    | oai_rt_rs::live::Error::Http { .. },
                ) => {
                    state.reported_end = true;
                    return Ok(Some(internal(
                        ContinuousLiveObservation::ObservationStreamEnded,
                    )));
                }
                Err(oai_rt_rs::live::Error::Provider(_)) => {
                    return diagnostic(LiveProviderDiagnosticCategory::BackendAdvisoryError)
                        .map(Some);
                }
                Err(oai_rt_rs::live::Error::MalformedEvent { raw, .. })
                    if matches!(
                        raw.get("type").and_then(serde_json::Value::as_str),
                        Some("session.started" | "session.updated" | "session.closed")
                    ) =>
                {
                    return Err(state.reject_identity());
                }
                Err(
                    oai_rt_rs::live::Error::Invalid(_)
                    | oai_rt_rs::live::Error::Json(_)
                    | oai_rt_rs::live::Error::MalformedEvent { .. },
                ) => {
                    return diagnostic(LiveProviderDiagnosticCategory::ProtocolInconsistency)
                        .map(Some);
                }
            }
        }
    }

    fn status(&self) -> LiveAdapterStatus {
        if self.identity_rejected.load(Ordering::Acquire) {
            return LiveAdapterStatus::Degraded {
                reason: LiveDegradationReason::Other {
                    detail: "public Live session identity rejected".into(),
                },
            };
        }
        if self.closed_confirmed.load(Ordering::Acquire) {
            return LiveAdapterStatus::Closed;
        }
        match self.sender.phase() {
            SessionPhase::Starting => LiveAdapterStatus::Opening,
            SessionPhase::Active if self.started.load(Ordering::Acquire) => {
                LiveAdapterStatus::Ready
            }
            SessionPhase::Active => LiveAdapterStatus::Opening,
            SessionPhase::Closing => LiveAdapterStatus::Closing,
            SessionPhase::Closed => LiveAdapterStatus::Closing,
            SessionPhase::Disconnected => LiveAdapterStatus::Degraded {
                reason: LiveDegradationReason::NetworkUnstable,
            },
        }
    }

    async fn close(&self) -> Result<(), LiveAdapterError> {
        // Observe physical closure without stealing queued TEXT/final usage
        // from the owner consuming next_observation.
        tokio::time::timeout(self.close_timeout, async {
            let mut close_sent = false;
            loop {
                if self.identity_rejected.load(Ordering::Acquire) {
                    return Err(transport_error(oai_rt_rs::live::Error::UnconfirmedClose));
                }
                if self.closed_confirmed.load(Ordering::Acquire) {
                    return Ok(());
                }
                match self.sender.phase() {
                    SessionPhase::Active if self.started.load(Ordering::Acquire) && !close_sent => {
                        close_sent = true;
                        self.sender
                            .send(ClientEvent::new(Command::Close))
                            .await
                            .map_err(transport_error)?;
                    }
                    SessionPhase::Disconnected => {
                        return Err(transport_error(oai_rt_rs::live::Error::UnconfirmedClose));
                    }
                    _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .map_err(|_| transport_error(oai_rt_rs::live::Error::UnconfirmedClose))?
    }

    fn capabilities(&self) -> LiveChannelCapabilities {
        LiveChannelCapabilities {
            audio_in: matches!(
                self.frontend.audio_ingress(),
                LiveAudioIngress::PcmWebSocket { .. }
            ),
            audio_out: matches!(
                self.frontend.audio_ingress(),
                LiveAudioIngress::PcmWebSocket { .. }
            ),
            transcript_supported: true,
            ..Default::default()
        }
    }

    async fn inject_observation(
        &self,
        observation: LiveAdapterObservation,
    ) -> Result<(), LiveAdapterError> {
        if !matches!(
            observation,
            LiveAdapterObservation::Error { .. }
                | LiveAdapterObservation::CommandRejected { .. }
                | LiveAdapterObservation::StatusChanged { .. }
        ) {
            return Err(LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::InternalError,
                message: "continuous adapter accepts only runtime control injection".into(),
            });
        }
        self.injected
            .try_send(observation)
            .map_err(|_| LiveAdapterError::TransportError {
                message: "runtime control observation queue is full or closed".into(),
            })
    }
}

fn project(
    state: &mut ReceiverState,
    frame: ServerFrame,
    role: oai_rt_rs::live::ConnectionRole,
) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
    if state.identity_rejected.load(Ordering::Acquire) {
        return Ok(None);
    }
    match &frame.event {
        ServerEvent::Started { session, .. } => {
            if session.id.is_empty()
                || session.id.len() > 128
                || state
                    .provider_session
                    .as_deref()
                    .is_some_and(|expected| expected != session.id)
            {
                return Err(state.reject_identity());
            }
            if state.started.load(Ordering::Acquire) {
                return Ok(None);
            }
            if state.provider_session.is_none() {
                state.provider_session = Some(session.id.clone());
            }
        }
        ServerEvent::Closed { session, .. } | ServerEvent::Updated { session, .. } => {
            if !state.started.load(Ordering::Acquire)
                || state.provider_session.as_deref() != Some(session.id.as_str())
            {
                return Err(state.reject_identity());
            }
        }
        _ => {}
    }
    if role == oai_rt_rs::live::ConnectionRole::Sideband
        && matches!(
            frame.event,
            ServerEvent::OutputAudioDelta { .. } | ServerEvent::InputAudio { .. }
        )
    {
        return Ok(None);
    }
    match frame.audio(role, AudioFormat::Pcm { rate: 24_000 }) {
        Ok(Some(audio)) => {
            return Ok(match audio.source {
                AudioSource::Output => Some(LiveAdapterObservation::AssistantAudioChunk {
                    data: audio.bytes,
                    sample_rate_hz: 24_000,
                    channels: 1,
                    response_id: None,
                    item_id: None,
                    content_index: None,
                }),
                AudioSource::ReflectedInput => None,
            });
        }
        Ok(None) => {}
        Err(_) => {
            return diagnostic(LiveProviderDiagnosticCategory::ProtocolInconsistency).map(Some);
        }
    }
    let usage = observe_voice_usage(&frame.event);
    let event = match frame.event {
        ServerEvent::Started { session, .. } => {
            state.started.store(true, Ordering::Release);
            ContinuousLiveObservation::ProviderStarted {
                provider_session: LiveProviderReference::new(session.id)
                    .map_err(|_| state.reject_identity())?,
            }
        }
        ServerEvent::InputTranscriptDelta {
            delta,
            start_ms,
            end_ms,
            ..
        } => transcript_event(LiveTranscriptDirection::Input, delta, start_ms, end_ms),
        ServerEvent::OutputTranscriptDelta {
            delta,
            start_ms,
            end_ms,
            ..
        } => transcript_event(LiveTranscriptDirection::Output, delta, start_ms, end_ms),
        ServerEvent::DelegationCreated {
            offset_ms,
            delegation,
            ..
        } if delegation.target == DelegationTarget::Client
            && offset_ms.is_finite()
            && offset_ms >= 0.0
            && delegation.id.len() <= 128 =>
        {
            let Ok(delegation) = LiveProviderReference::new(delegation.id) else {
                return diagnostic(LiveProviderDiagnosticCategory::ProtocolInconsistency).map(Some);
            };
            ContinuousLiveObservation::ClientDelegation {
                delegation,
                offset_ms,
            }
        }
        ServerEvent::UsageUpdated { .. } => match usage {
            Ok(Some(snapshot)) => ContinuousLiveObservation::VoiceUsage(snapshot),
            Err(_) => ContinuousLiveObservation::VoiceUsage(LiveUsageSnapshot::Disputed {
                last_valid_seconds: None,
                reason: LiveUsageDispute::InvalidDuration,
            }),
            Ok(None) => {
                return Err(LiveAdapterError::ProviderError {
                    code: LiveAdapterErrorCode::InternalError,
                    message: "usage event has no usage projection".into(),
                });
            }
        },
        ServerEvent::Closed { .. } => ContinuousLiveObservation::ProviderClosed {
            usage: match usage {
                Ok(Some(snapshot)) => snapshot,
                Err(_) => LiveUsageSnapshot::Disputed {
                    last_valid_seconds: None,
                    reason: LiveUsageDispute::InvalidDuration,
                },
                Ok(None) => {
                    return Err(LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message: "closed event has no usage projection".into(),
                    });
                }
            },
        },
        ServerEvent::Error { .. } | ServerEvent::Info { .. } => {
            return diagnostic(LiveProviderDiagnosticCategory::BackendAdvisoryError).map(Some);
        }
        ServerEvent::InstructionsAppended { .. }
        | ServerEvent::ThinkingAppended { .. }
        | ServerEvent::CommentaryAppended { .. } => {
            return diagnostic(LiveProviderDiagnosticCategory::UncorrelatedContextAcknowledgment)
                .map(Some);
        }
        ServerEvent::InputAudioMuted { .. } | ServerEvent::InputAudioUnmuted { .. } => {
            return Ok(None);
        }
        ServerEvent::Updated { .. }
        | ServerEvent::DelegationCreated { .. }
        | ServerEvent::Response { .. }
        | ServerEvent::DtmfReceived { .. }
        | ServerEvent::DtmfSend { .. }
        | ServerEvent::Ringing { .. }
        | ServerEvent::Answered { .. }
        | ServerEvent::TransportFailed { .. }
        | ServerEvent::Unknown
        | ServerEvent::OutputAudioDelta { .. }
        | ServerEvent::InputAudio { .. } => {
            return diagnostic(LiveProviderDiagnosticCategory::UnsupportedProviderEvent).map(Some);
        }
    };
    if matches!(event, ContinuousLiveObservation::ProviderClosed { .. }) {
        state.closed_confirmed.store(true, Ordering::Release);
    }
    Ok(Some(internal(event)))
}
