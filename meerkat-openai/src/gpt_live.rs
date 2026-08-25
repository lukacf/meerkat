//! Experimental ChatGPT-backed GPT Live broker adapter.
//!
//! This module owns provider mechanics only. It validates a registry-minted
//! realtime target, injects the resolved ChatGPT credential into the private
//! transport, and returns an opaque sideband session plus the SDP answer that
//! the caller must apply to its browser-owned WebRTC peer connection.
//!
//! Transcript admission, delegation meaning, channel policy, recovery, and
//! model selection remain outside this module.

use meerkat_core::model_profile::catalog::ModelReleaseStage;
use meerkat_core::{ModelProfileWitness, Provider, ProviderAuthMetadata};
use meerkat_llm_core::provider_runtime::errors::ProviderClientError;
use meerkat_llm_core::provider_runtime::{
    AdmittedExperimentalRealtimeTarget, ExperimentalRealtimeAdmissionRetention,
    NormalizedBackendKind, ResolvedRealtimeTarget,
};
#[cfg(feature = "test-realtime-fixtures")]
use oai_rt_rs::experimental::gpt_live::GptLiveEndpoints;
use oai_rt_rs::experimental::gpt_live::{
    CallSession, ClientEvent, ContextChannel, CreateCallRequest, Delegation,
    DelegationContextAppend, ExtraFields, FunctionTool, GptLiveCredentials, GptLiveTransport,
    InputTextContent, ResponsesConfig, ResponsesDelegation, ServerEvent, SessionAudio,
    SessionAudioOutput, SessionContextAppend, SidebandHeaders, SidebandReceiver, SidebandSender,
    TransportError,
};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::OpenAiBackendKind;

pub const GPT_LIVE_RESPONSES_BRIDGE_TOOL: &str = "invoke_meerkat";

#[allow(
    dead_code,
    reason = "FunctionBridge remains closed until Gate 0 qualifies raw Responses events"
)]
const GPT_LIVE_RESPONSES_BRIDGE_DESCRIPTION: &str =
    "Delegate this request to the channel-bound Meerkat agent.";
#[allow(
    dead_code,
    reason = "FunctionBridge remains closed until Gate 0 qualifies raw Responses events"
)]
const GPT_LIVE_RESPONSES_BRIDGE_INSTRUCTIONS: &str =
    "Use invoke_meerkat when the request requires the channel-bound Meerkat agent.";

#[allow(
    dead_code,
    reason = "FunctionBridge remains closed until Gate 0 qualifies raw Responses events"
)]
fn gpt_live_responses_bridge_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "request": { "type": "string" }
        },
        "required": ["request"],
        "additionalProperties": false
    })
}

/// Catalog-bound configuration for the server-owned Responses bridge.
///
/// The caller supplies only a registry-minted model witness. Instructions,
/// tool identity, description, and arguments schema are fixed by this profile;
/// no caller or operator prose can enter the provider session through it.
#[derive(Clone)]
pub struct GptLiveResponsesSessionConfig {
    responses: ResponsesConfig,
}

impl GptLiveResponsesSessionConfig {
    #[allow(
        dead_code,
        reason = "FunctionBridge remains closed until Gate 0 qualifies raw Responses events"
    )]
    pub(crate) fn try_from_catalog_model(
        model: &ModelProfileWitness,
    ) -> Result<Self, GptLiveBrokerError> {
        if model.provider() != Provider::OpenAI || model.profile().realtime {
            return Err(GptLiveBrokerError::InvalidResponsesProfile);
        }
        Ok(Self {
            responses: ResponsesConfig {
                model: model.model().to_string(),
                instructions: Some(GPT_LIVE_RESPONSES_BRIDGE_INSTRUCTIONS.to_string()),
                tools: vec![FunctionTool::new(
                    GPT_LIVE_RESPONSES_BRIDGE_TOOL,
                    GPT_LIVE_RESPONSES_BRIDGE_DESCRIPTION,
                    gpt_live_responses_bridge_parameters(),
                    ExtraFields::new(),
                )],
                extra: ExtraFields::new(),
            },
        })
    }

    fn into_delegation(self) -> Delegation {
        Delegation::Responses(ResponsesDelegation::new(self.responses, ExtraFields::new()))
    }
}

impl std::fmt::Debug for GptLiveResponsesSessionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GptLiveResponsesSessionConfig")
            .field("model", &"<catalog-bound>")
            .field("instructions", &"<catalog-owned-redacted>")
            .field("tool", &GPT_LIVE_RESPONSES_BRIDGE_TOOL)
            .finish()
    }
}

/// Provider-owned mechanical configuration for one browser WebRTC bootstrap.
#[derive(Clone)]
pub struct GptLiveBrokerOpenConfig {
    offer_sdp: String,
    voice: String,
    responses: Option<GptLiveResponsesSessionConfig>,
}

impl std::fmt::Debug for GptLiveBrokerOpenConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GptLiveBrokerOpenConfig")
            .field("offer_sdp", &"<redacted>")
            .field("voice", &"<redacted>")
            .field("responses", &self.responses)
            .finish()
    }
}

impl GptLiveBrokerOpenConfig {
    /// Construct the minimum verified private call shape.
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
        Ok(Self {
            offer_sdp,
            voice,
            responses: None,
        })
    }

    /// Select the catalog-bound Responses function bridge.
    ///
    /// There is deliberately no client-delegation variant. Client-context is a
    /// separate capability and cannot be silently substituted for this mode.
    #[must_use]
    pub fn with_responses_session(mut self, responses: GptLiveResponsesSessionConfig) -> Self {
        self.responses = Some(responses);
        self
    }
}

/// Sanitized terminal classification for private broker mechanics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptLiveBrokerTerminalClass {
    Configuration,
    Protocol,
    Http,
    WebSocket,
    Closed,
}

/// Opaque local identity for one append attempt.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GptLiveAppendToken(u64);

impl std::fmt::Debug for GptLiveAppendToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveAppendToken(<local>)")
    }
}

/// Opaque delegation reference that can only be minted by this adapter.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GptLiveDelegationRef(String);

impl GptLiveDelegationRef {
    /// Borrow only at the facade boundary that seals the provider-neutral
    /// opaque delegation reference. The value must never be logged.
    #[doc(hidden)]
    #[must_use]
    pub fn __opaque_provider_id(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GptLiveTranscriptItemRef(String);

impl GptLiveTranscriptItemRef {
    #[doc(hidden)]
    #[must_use]
    pub fn __opaque_provider_id(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GptLiveTranscriptItemRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveTranscriptItemRef(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GptLiveTurnRef(String);

impl GptLiveTurnRef {
    #[doc(hidden)]
    #[must_use]
    pub fn __opaque_provider_id(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GptLiveTurnRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveTurnRef(<redacted>)")
    }
}

impl std::fmt::Debug for GptLiveDelegationRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveDelegationRef(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptLiveTurnRole {
    User,
    Assistant,
    Unknown,
}

impl GptLiveTurnRole {
    fn from_provider_role(role: &str) -> Self {
        match role {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => Self::Unknown,
        }
    }
}

/// Sanitized provider observations emitted by the private sideband adapter.
///
/// No provider call, session, turn, transcript-item, handoff, or delegation
/// identifier is exposed. Text is semantic observation content, not a raw
/// private payload.
#[derive(Clone, PartialEq, Eq)]
pub enum GptLiveBrokerObservation {
    SessionReady,
    SessionContextAppendAcknowledged {
        token: GptLiveAppendToken,
    },
    UserTranscriptFragment {
        item: GptLiveTranscriptItemRef,
        text: String,
    },
    AssistantTranscriptFragment {
        item: GptLiveTranscriptItemRef,
        text: String,
    },
    TurnStarted {
        turn: GptLiveTurnRef,
        role: GptLiveTurnRole,
    },
    TurnSnapshotDelta {
        turn: GptLiveTurnRef,
        delta: String,
    },
    TurnFinished {
        turn: GptLiveTurnRef,
        role: GptLiveTurnRole,
        transcript: String,
    },
    DelegationActionableInputUnsupported {
        delegation: GptLiveDelegationRef,
    },
    DelegationContextAppendAcknowledged {
        token: GptLiveAppendToken,
    },
    UnsupportedPrivateEvent,
}

impl std::fmt::Debug for GptLiveBrokerObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::SessionReady => "session_ready",
            Self::SessionContextAppendAcknowledged { .. } => "session_context_append_acknowledged",
            Self::UserTranscriptFragment { .. } => "user_transcript_fragment",
            Self::AssistantTranscriptFragment { .. } => "assistant_transcript_fragment",
            Self::TurnStarted { .. } => "turn_started",
            Self::TurnSnapshotDelta { .. } => "turn_snapshot_delta",
            Self::TurnFinished { .. } => "turn_finished",
            Self::DelegationActionableInputUnsupported { .. } => {
                "delegation_actionable_input_unsupported"
            }
            Self::DelegationContextAppendAcknowledged { .. } => {
                "delegation_context_append_acknowledged"
            }
            Self::UnsupportedPrivateEvent => "unsupported_private_event",
        };
        formatter
            .debug_struct("GptLiveBrokerObservation")
            .field("kind", &kind)
            .field("payload", &"<redacted>")
            .finish()
    }
}

/// Sanitized failure surface for browser bootstrap and sideband mechanics.
#[derive(Error)]
pub enum GptLiveBrokerError {
    #[error("GPT Live browser bootstrap requires a non-empty SDP offer")]
    MissingOfferSdp,
    #[error("GPT Live browser bootstrap requires a non-empty voice")]
    MissingVoice,
    #[error("GPT Live Responses bridge requires a catalogued non-realtime OpenAI model")]
    InvalidResponsesProfile,
    #[error("GPT Live context append requires non-empty text")]
    MissingContext,
    #[error("a GPT Live context append is already awaiting acknowledgement")]
    AppendInFlight,
    #[error("GPT Live append delivery is ambiguous and must not be retried blindly")]
    AppendDeliveryAmbiguous { token: GptLiveAppendToken },
    #[error("GPT Live private transport terminated")]
    Transport { class: GptLiveBrokerTerminalClass },
}

impl From<TransportError> for GptLiveBrokerError {
    fn from(source: TransportError) -> Self {
        let class = match source {
            TransportError::Codec(_)
            | TransportError::UnexpectedContentType
            | TransportError::MissingCallLocation
            | TransportError::InvalidCallLocation
            | TransportError::OversizedAnswer
            | TransportError::InvalidAnswerEncoding => GptLiveBrokerTerminalClass::Protocol,
            TransportError::Http(_) | TransportError::UnexpectedStatus(_) => {
                GptLiveBrokerTerminalClass::Http
            }
            TransportError::WebSocket(_) => GptLiveBrokerTerminalClass::WebSocket,
            TransportError::InvalidEndpoint | TransportError::InvalidHeader(_) => {
                GptLiveBrokerTerminalClass::Configuration
            }
            TransportError::Closed => GptLiveBrokerTerminalClass::Closed,
        };
        Self::Transport { class }
    }
}

impl std::fmt::Debug for GptLiveBrokerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOfferSdp => formatter.write_str("MissingOfferSdp"),
            Self::MissingVoice => formatter.write_str("MissingVoice"),
            Self::InvalidResponsesProfile => formatter.write_str("InvalidResponsesProfile"),
            Self::MissingContext => formatter.write_str("MissingContext"),
            Self::AppendInFlight => formatter.write_str("AppendInFlight"),
            Self::AppendDeliveryAmbiguous { token } => formatter
                .debug_struct("AppendDeliveryAmbiguous")
                .field("token", token)
                .finish(),
            Self::Transport { class } => formatter
                .debug_struct("Transport")
                .field("class", class)
                .finish(),
        }
    }
}

/// Concrete provider factory admitted from one exact resolved realtime target.
pub struct GptLiveBrokerFactory {
    model: String,
    transport: GptLiveTransport,
    credentials: GptLiveCredentials,
    _admission: GptLiveFactoryAdmissionRetention,
}

struct GptLiveFactoryAdmissionRetention {
    _admitted: Option<ExperimentalRealtimeAdmissionRetention>,
}

impl std::fmt::Debug for GptLiveBrokerFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GptLiveBrokerFactory")
            .field("model", &"<registry-admitted>")
            .field("transport", &"<private transport>")
            .field("credentials", &"<redacted>")
            .finish()
    }
}

impl GptLiveBrokerFactory {
    /// Build the experimental broker only from catalog and binding evidence.
    ///
    /// A model-name string alone cannot select this path. The witness must
    /// classify the exact identity as experimental and realtime-capable, and
    /// the resolved connection must be the ChatGPT backend with inline OAuth
    /// credential material.
    pub fn try_from_admitted_target(
        admitted: AdmittedExperimentalRealtimeTarget,
    ) -> Result<Self, ProviderClientError> {
        let (target, admission) = admitted.into_parts();
        let (model, credentials) = Self::admit_target(target)?;
        let transport = GptLiveTransport::try_new().map_err(|_| {
            ProviderClientError::ClientInit(
                "failed to construct experimental GPT Live transport".to_string(),
            )
        })?;
        Ok(Self {
            model,
            transport,
            credentials,
            _admission: GptLiveFactoryAdmissionRetention {
                _admitted: Some(admission),
            },
        })
    }

    /// Test-only external endpoint injection after real admission has been
    /// consumed into provider custody. This retains the exact admission
    /// witness and changes only the private HTTP and WebSocket destinations.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    pub fn __try_from_admitted_target_with_endpoints(
        admitted: AdmittedExperimentalRealtimeTarget,
        call_url: &str,
        sideband_base_url: &str,
    ) -> Result<Self, ProviderClientError> {
        let (target, admission) = admitted.into_parts();
        let (model, credentials) = Self::admit_target(target)?;
        let endpoints = GptLiveEndpoints::new(call_url, sideband_base_url).map_err(|_| {
            ProviderClientError::ClientInit(
                "failed to construct experimental GPT Live test endpoints".to_string(),
            )
        })?;
        let transport = GptLiveTransport::with_endpoints(endpoints).map_err(|_| {
            ProviderClientError::ClientInit(
                "failed to construct experimental GPT Live test transport".to_string(),
            )
        })?;
        Ok(Self {
            model,
            transport,
            credentials,
            _admission: GptLiveFactoryAdmissionRetention {
                _admitted: Some(admission),
            },
        })
    }

    #[cfg(test)]
    fn from_target_with_transport(
        target: ResolvedRealtimeTarget,
        transport: GptLiveTransport,
    ) -> Result<Self, ProviderClientError> {
        let (model, credentials) = Self::admit_target(target)?;
        Ok(Self {
            model,
            transport,
            credentials,
            _admission: GptLiveFactoryAdmissionRetention { _admitted: None },
        })
    }

    fn admit_target(
        target: ResolvedRealtimeTarget,
    ) -> Result<(String, GptLiveCredentials), ProviderClientError> {
        let profile = target.profile().profile();
        if profile.release_stage != ModelReleaseStage::Experimental {
            return Err(ProviderClientError::MissingFeature(
                "openai-experimental-gpt-live-model",
            ));
        }
        if !profile.realtime {
            return Err(ProviderClientError::MissingFeature(
                "openai-experimental-gpt-live-realtime",
            ));
        }

        let (identity, _, connection) = target.into_parts();
        if !matches!(
            connection.backend,
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend)
        ) {
            return Err(ProviderClientError::MissingFeature(
                "openai-experimental-gpt-live-chatgpt-backend",
            ));
        }
        if connection.resolved_authorizer().is_some() {
            return Err(ProviderClientError::MissingFeature(
                "openai-experimental-gpt-live-authorizer-auth",
            ));
        }
        let bearer_token = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let metadata = connection.auth_lease.metadata();
        let account_id = match metadata.provider_metadata {
            Some(ProviderAuthMetadata::OpenAi(ref metadata)) => metadata.account_id.clone(),
            _ => metadata.account_id.clone(),
        };
        let credentials = GptLiveCredentials::new(
            bearer_token,
            SidebandHeaders {
                account_id,
                ..SidebandHeaders::default()
            },
        );

        Ok((identity.model, credentials))
    }

    /// Create the private call and connect its sideband before returning.
    ///
    /// The answer SDP remains opaque browser bootstrap data. The returned
    /// session keeps provider call identity and private events inside the
    /// OpenAI adapter boundary.
    pub async fn open(
        &self,
        config: GptLiveBrokerOpenConfig,
    ) -> Result<GptLiveBrokerBootstrap, GptLiveBrokerError> {
        let request = self.call_request(config);
        let created = self
            .transport
            .create_call(&request, &self.credentials)
            .await?;
        let sideband = self
            .transport
            .connect_sideband(&created.call_id, &self.credentials)
            .await?;
        let (sender, receiver) = sideband.split();
        Ok(GptLiveBrokerBootstrap {
            answer_sdp: created.answer_sdp,
            session: GptLiveBrokerSession {
                sender,
                receiver: Mutex::new(receiver),
                state: Mutex::new(GptLiveBrokerSessionState {
                    next_append_token: 1,
                    pending_session_append: None,
                    pending_delegation_append: None,
                }),
            },
        })
    }

    fn call_request(&self, config: GptLiveBrokerOpenConfig) -> CreateCallRequest {
        CreateCallRequest {
            sdp: config.offer_sdp,
            session: CallSession {
                model: self.model.clone(),
                audio: SessionAudio {
                    output: SessionAudioOutput {
                        voice: config.voice,
                        extra: ExtraFields::new(),
                    },
                    extra: ExtraFields::new(),
                },
                delegation: config
                    .responses
                    .map(GptLiveResponsesSessionConfig::into_delegation),
                instructions: None,
                extra: ExtraFields::new(),
            },
        }
    }
}

/// Browser-facing SDP answer paired with a provider-owned opaque broker session.
pub struct GptLiveBrokerBootstrap {
    answer_sdp: String,
    session: GptLiveBrokerSession,
}

impl std::fmt::Debug for GptLiveBrokerBootstrap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GptLiveBrokerBootstrap")
            .field("answer_sdp", &"<redacted>")
            .field("session", &self.session)
            .finish()
    }
}

impl GptLiveBrokerBootstrap {
    /// Borrow the answer SDP for the browser peer connection.
    #[must_use]
    pub fn answer_sdp(&self) -> &str {
        &self.answer_sdp
    }

    /// Transfer the opaque broker session to its provider-owned host.
    #[must_use]
    pub fn into_parts(self) -> (String, GptLiveBrokerSession) {
        (self.answer_sdp, self.session)
    }
}

/// Opaque connected sideband handle.
///
/// Private protocol events and provider call IDs are intentionally not
/// exposed. Captured private events are lowered to sanitized observations
/// inside this crate.
pub struct GptLiveBrokerSession {
    sender: SidebandSender,
    receiver: Mutex<SidebandReceiver>,
    state: Mutex<GptLiveBrokerSessionState>,
}

struct GptLiveBrokerSessionState {
    next_append_token: u64,
    pending_session_append: Option<GptLiveAppendToken>,
    pending_delegation_append: Option<(GptLiveDelegationRef, GptLiveAppendToken)>,
}

impl std::fmt::Debug for GptLiveBrokerSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveBrokerSession(<connected>)")
    }
}

impl GptLiveBrokerSession {
    /// Wait for provider readiness and seed one ordered canonical commentary
    /// envelope before the caller exposes the session as ready. Any missing,
    /// ambiguous, or mismatched acknowledgement closes the partial session;
    /// this method never retries.
    pub async fn await_ready_and_seed_session_context(
        &self,
        commentary: Option<String>,
    ) -> Result<(), GptLiveBrokerError> {
        let seeded = async {
            if !matches!(
                self.next_observation().await?,
                Some(GptLiveBrokerObservation::SessionReady)
            ) {
                return Err(GptLiveBrokerError::Transport {
                    class: GptLiveBrokerTerminalClass::Protocol,
                });
            }
            if let Some(commentary) = commentary {
                let token = self.append_session_context(commentary).await?;
                if !matches!(
                    self.next_observation().await?,
                    Some(GptLiveBrokerObservation::SessionContextAppendAcknowledged {
                        token: acknowledged,
                    }) if acknowledged == token
                ) {
                    return Err(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::Protocol,
                    });
                }
            }
            Ok(())
        }
        .await;
        if seeded.is_err() {
            let _ = self.close().await;
        }
        seeded
    }

    /// Append canonical Meerkat context without granting it automatic speech.
    ///
    /// Only one session-context append may be unacknowledged because the
    /// private acknowledgement carries no caller correlation. A send failure
    /// is classified as ambiguous and retains the pending token so callers
    /// cannot accidentally retry the same append.
    pub async fn append_session_context(
        &self,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = {
            let mut state = self.state.lock().await;
            if state.pending_session_append.is_some() {
                return Err(GptLiveBrokerError::AppendInFlight);
            }
            let token = state.allocate_append_token();
            state.pending_session_append = Some(token);
            token
        };
        let event = ClientEvent::SessionContextAppend(SessionContextAppend {
            channel: Some(ContextChannel::Commentary),
            content: vec![context_content(text)],
            extra: ExtraFields::new(),
        });
        if self.sender.send(&event).await.is_err() {
            return Err(GptLiveBrokerError::AppendDeliveryAmbiguous { token });
        }
        Ok(token)
    }

    /// Append executor context to an observed delegation.
    ///
    /// The provider identifier remains inside the opaque delegation reference.
    /// Results and progress are always appended as commentary/context. The
    /// live model alone decides whether and how to speak from that context.
    pub async fn append_delegation_context(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = {
            let mut state = self.state.lock().await;
            if state.pending_delegation_append.is_some() {
                return Err(GptLiveBrokerError::AppendInFlight);
            }
            let token = state.allocate_append_token();
            state.pending_delegation_append = Some((delegation.clone(), token));
            token
        };
        let event = ClientEvent::DelegationContextAppend(DelegationContextAppend {
            delegation_item_id: delegation.0.clone(),
            channel: Some(ContextChannel::Commentary),
            content: vec![context_content(text)],
            extra: ExtraFields::new(),
        });
        if self.sender.send(&event).await.is_err() {
            return Err(GptLiveBrokerError::AppendDeliveryAmbiguous { token });
        }
        Ok(token)
    }

    /// Receive and sanitize one captured private sideband event.
    ///
    /// Delegation identity is retained only as an opaque return handle. The
    /// adapter deliberately reports actionable handoff input as unsupported
    /// until the no-Codex Gate 0 proves an exact source and join.
    pub async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
        let event = self.receiver.lock().await.next_event().await?;
        let Some(event) = event else {
            return Ok(None);
        };
        let observation = match event {
            ServerEvent::SessionStarted(_) => GptLiveBrokerObservation::SessionReady,
            ServerEvent::SessionContextAppended(_) => {
                let token = self
                    .state
                    .lock()
                    .await
                    .pending_session_append
                    .take()
                    .ok_or(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::Protocol,
                    })?;
                GptLiveBrokerObservation::SessionContextAppendAcknowledged { token }
            }
            ServerEvent::InputTranscriptAdded(event) => {
                GptLiveBrokerObservation::UserTranscriptFragment {
                    item: GptLiveTranscriptItemRef(event.item.id),
                    text: event.item.text,
                }
            }
            ServerEvent::OutputTranscriptAdded(event) => {
                GptLiveBrokerObservation::AssistantTranscriptFragment {
                    item: GptLiveTranscriptItemRef(event.item.id),
                    text: event.item.text,
                }
            }
            ServerEvent::TurnCreated(event) => {
                let role = GptLiveTurnRole::from_provider_role(&event.turn.role);
                GptLiveBrokerObservation::TurnStarted {
                    turn: GptLiveTurnRef(event.turn.id),
                    role,
                }
            }
            ServerEvent::TurnDelta(event) => GptLiveBrokerObservation::TurnSnapshotDelta {
                turn: GptLiveTurnRef(event.turn_id),
                delta: event.delta,
            },
            ServerEvent::TurnDone(event) => {
                let role = GptLiveTurnRole::from_provider_role(&event.turn.role);
                GptLiveBrokerObservation::TurnFinished {
                    turn: GptLiveTurnRef(event.turn.id),
                    role,
                    transcript: event.turn.transcript,
                }
            }
            ServerEvent::DelegationCreated(event) => {
                GptLiveBrokerObservation::DelegationActionableInputUnsupported {
                    delegation: GptLiveDelegationRef(event.item.id),
                }
            }
            ServerEvent::DelegationContextAppended(event) => {
                let (delegation, token) = self
                    .state
                    .lock()
                    .await
                    .pending_delegation_append
                    .take()
                    .ok_or(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::Protocol,
                    })?;
                if delegation.0 != event.delegation_item_id {
                    return Err(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::Protocol,
                    });
                }
                GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token }
            }
            ServerEvent::Unknown(_) => GptLiveBrokerObservation::UnsupportedPrivateEvent,
        };
        Ok(Some(observation))
    }

    /// Close the private sideband without exposing its wire identity.
    pub async fn close(&self) -> Result<(), GptLiveBrokerError> {
        self.sender.close().await.map_err(Into::into)
    }
}

impl GptLiveBrokerSessionState {
    fn allocate_append_token(&mut self) -> GptLiveAppendToken {
        let token = GptLiveAppendToken(self.next_append_token);
        self.next_append_token = self.next_append_token.saturating_add(1);
        token
    }
}

fn require_context(text: impl Into<String>) -> Result<String, GptLiveBrokerError> {
    let text = text.into();
    if text.trim().is_empty() {
        return Err(GptLiveBrokerError::MissingContext);
    }
    Ok(text)
}

fn context_content(text: String) -> InputTextContent {
    InputTextContent {
        content_type: "input_text".to_string(),
        text,
        extra: ExtraFields::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
    use axum::http::{Response, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use meerkat_core::{
        AuthMetadata, Config, ModelRegistry, Provider, SessionLlmIdentity,
        connection::BackendProfile,
    };
    use meerkat_llm_core::provider_runtime::{ResolvedConnection, StaticLease};
    use oai_rt_rs::experimental::gpt_live::GptLiveEndpoints;
    use serde_json::{Value, json};
    use std::sync::Mutex;

    fn realtime_target(
        release_stage: ModelReleaseStage,
        backend_kind: OpenAiBackendKind,
    ) -> ResolvedRealtimeTarget {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("canonical model registry");
        let entry = registry
            .entries_for_provider(Provider::OpenAI)
            .find(|entry| {
                entry.release_stage == release_stage
                    && registry
                        .profile_for_provider(Provider::OpenAI, &entry.id)
                        .is_some_and(|profile| profile.realtime)
            })
            .expect("realtime model for requested release stage");
        let identity = SessionLlmIdentity {
            model: entry.id.clone(),
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
                options: serde_json::Value::Null,
                server: None,
            }),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "credential-secret".to_string(),
                AuthMetadata::default(),
                None,
                "openai:test",
            )),
        };
        ResolvedRealtimeTarget::new(identity, witness, connection).expect("matching target")
    }

    fn responses_model() -> ModelProfileWitness {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("canonical model registry");
        let entry = registry
            .entries_for_provider(Provider::OpenAI)
            .find(|entry| {
                registry
                    .profile_for_provider(Provider::OpenAI, &entry.id)
                    .is_some_and(|profile| !profile.realtime)
            })
            .expect("catalogued non-realtime OpenAI model");
        registry
            .profile_witness_for_provider(Provider::OpenAI, &entry.id)
            .expect("catalog model witness")
    }

    #[derive(Default)]
    struct Capture {
        call_body: Option<Value>,
        client_events: Vec<Value>,
    }

    type SharedCapture = Arc<Mutex<Capture>>;

    async fn create_call(State(capture): State<SharedCapture>, body: Bytes) -> Response<Body> {
        capture.lock().expect("capture lock").call_body = serde_json::from_slice(&body).ok();
        Response::builder()
            .status(StatusCode::CREATED)
            .header("content-type", "text/plain")
            .header("location", "/v1/realtime/calls/rtc_private_fixture")
            .body(Body::from("v=0\r\nPRIVATE_ANSWER_SDP"))
            .expect("create call response")
    }

    async fn connect_sideband(
        State(capture): State<SharedCapture>,
        upgrade: WebSocketUpgrade,
    ) -> impl IntoResponse {
        upgrade.on_upgrade(move |socket| serve_sideband(socket, capture))
    }

    async fn connect_sideband_reject_seed(
        State(capture): State<SharedCapture>,
        upgrade: WebSocketUpgrade,
    ) -> impl IntoResponse {
        upgrade.on_upgrade(move |socket| serve_sideband_reject_seed(socket, capture))
    }

    async fn serve_sideband_reject_seed(mut socket: WebSocket, capture: SharedCapture) {
        socket
            .send(AxumMessage::Text(
                json!({
                    "type": "session.started",
                    "session": {
                        "id": "rtc_private_fixture",
                        "expires_at": 0,
                        "status": "active"
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send fixture readiness");
        if let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
            capture
                .lock()
                .expect("capture lock")
                .client_events
                .push(serde_json::from_str(&text).expect("session append JSON"));
        }
        socket
            .send(AxumMessage::Text(
                json!({
                    "type": "session.ended",
                    "session_id": "rtc_private_fixture",
                    "reason": "fixture_seed_rejected"
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send terminal observation instead of seed acknowledgement");
        let _ = socket.recv().await;
    }

    async fn serve_sideband(mut socket: WebSocket, capture: SharedCapture) {
        socket
            .send(AxumMessage::Text(
                json!({
                    "type": "session.started",
                    "session": {
                        "id": "rtc_private_fixture",
                        "expires_at": 0,
                        "status": "active"
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send fixture readiness");

        if let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
            capture
                .lock()
                .expect("capture lock")
                .client_events
                .push(serde_json::from_str(&text).expect("session append JSON"));
            socket
                .send(AxumMessage::Text(
                    json!({
                        "type": "session.context.appended",
                        "start_ms": 2,
                        "end_ms": 3
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("session append acknowledgement");
        }

        for event in [
            json!({
                "type": "input_transcript.added",
                "start_ms": 4,
                "end_ms": 5,
                "item": { "id": "private_input_item", "type": "transcript", "text": "input fragment" }
            }),
            json!({
                "type": "output_transcript.added",
                "start_ms": 5,
                "end_ms": 6,
                "item": { "id": "private_output_item", "type": "transcript", "text": "output fragment" }
            }),
            json!({
                "type": "turn.created",
                "turn": { "id": "private_user_turn", "role": "user", "start_ms": 6, "end_ms": 0, "transcript": "" }
            }),
            json!({
                "type": "turn.delta",
                "turn_id": "private_user_turn",
                "start_ms": 6,
                "end_ms": 7,
                "delta": "unqualified snapshot"
            }),
            json!({
                "type": "turn.done",
                "turn": { "id": "private_user_turn", "role": "user", "start_ms": 6, "end_ms": 8, "transcript": "authoritative user final" }
            }),
            json!({
                "type": "turn.created",
                "turn": { "id": "private_assistant_turn", "role": "assistant", "start_ms": 9, "end_ms": 0, "transcript": "" }
            }),
            json!({
                "type": "turn.done",
                "turn": { "id": "private_assistant_turn", "role": "assistant", "start_ms": 9, "end_ms": 10, "transcript": "authoritative assistant final" }
            }),
        ] {
            socket
                .send(AxumMessage::Text(event.to_string().into()))
                .await
                .expect("send role-bearing transcript fixture event");
        }

        socket
            .send(AxumMessage::Text(
                json!({
                    "type": "delegation.created",
                    "offset_ms": 1,
                    "item": {
                        "id": "private_delegation_id",
                        "type": "delegation",
                        "target": "client",
                        "handoff_id": "private_handoff_id",
                        "user_bidi_turn_id": "private_turn_id",
                        "content": []
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send fixture delegation after seed acknowledgement");

        if let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
            capture
                .lock()
                .expect("capture lock")
                .client_events
                .push(serde_json::from_str(&text).expect("delegation append JSON"));
            socket
                .send(AxumMessage::Text(
                    json!({
                        "type": "delegation.context.appended",
                        "delegation_item_id": "private_delegation_id",
                        "start_ms": 4,
                        "end_ms": 5
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("delegation append acknowledgement");
        }
        let _ = socket.send(AxumMessage::Close(None)).await;
    }

    async fn local_transport() -> (GptLiveTransport, SharedCapture, tokio::task::JoinHandle<()>) {
        let capture = Arc::new(Mutex::new(Capture::default()));
        let app = Router::new()
            .route("/backend-api/codex/realtime/calls", post(create_call))
            .route("/v1/live/{call_id}", get(connect_sideband))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fixture transport");
        });
        let endpoints = GptLiveEndpoints::new(
            &format!(
                "http://{address}/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas"
            ),
            &format!("ws://{address}/v1/live/"),
        )
        .expect("fixture endpoints");
        (
            GptLiveTransport::with_endpoints(endpoints).expect("fixture transport"),
            capture,
            server,
        )
    }

    async fn local_transport_rejecting_seed()
    -> (GptLiveTransport, SharedCapture, tokio::task::JoinHandle<()>) {
        let capture = Arc::new(Mutex::new(Capture::default()));
        let app = Router::new()
            .route("/backend-api/codex/realtime/calls", post(create_call))
            .route("/v1/live/{call_id}", get(connect_sideband_reject_seed))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve rejecting fixture transport");
        });
        let endpoints = GptLiveEndpoints::new(
            &format!(
                "http://{address}/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas"
            ),
            &format!("ws://{address}/v1/live/"),
        )
        .expect("fixture endpoints");
        (
            GptLiveTransport::with_endpoints(endpoints).expect("fixture transport"),
            capture,
            server,
        )
    }

    #[test]
    fn open_config_rejects_blank_mechanical_inputs_without_echoing_them() {
        assert!(matches!(
            GptLiveBrokerOpenConfig::new("   ", "cove"),
            Err(GptLiveBrokerError::MissingOfferSdp)
        ));
        assert!(matches!(
            GptLiveBrokerOpenConfig::new("v=0", "   "),
            Err(GptLiveBrokerError::MissingVoice)
        ));
    }

    #[test]
    fn open_config_debug_redacts_sdp_voice_and_catalog_responses_profile() {
        let responses = GptLiveResponsesSessionConfig::try_from_catalog_model(&responses_model())
            .expect("catalog responses profile");
        let config = GptLiveBrokerOpenConfig::new("sdp-secret", "voice-secret")
            .unwrap()
            .with_responses_session(responses);
        let debug = format!("{config:?}");
        assert!(!debug.contains("sdp-secret"));
        assert!(!debug.contains("voice-secret"));
        assert!(debug.contains("catalog-owned-redacted"));
        assert!(!debug.contains(GPT_LIVE_RESPONSES_BRIDGE_INSTRUCTIONS));
    }

    #[test]
    fn function_bridge_open_config_can_only_produce_responses_delegation() {
        let target = realtime_target(
            ModelReleaseStage::Experimental,
            OpenAiBackendKind::ChatGptBackend,
        );
        let factory = GptLiveBrokerFactory::from_target_with_transport(
            target,
            GptLiveTransport::try_new().expect("test transport"),
        )
        .expect("experimental ChatGPT target");
        let responses = GptLiveResponsesSessionConfig::try_from_catalog_model(&responses_model())
            .expect("catalog responses profile");
        let request = factory.call_request(
            GptLiveBrokerOpenConfig::new("PRIVATE_OFFER_SDP", "cove")
                .expect("open config")
                .with_responses_session(responses),
        );
        let wire = serde_json::to_value(request).expect("serialize call request");

        assert_eq!(wire["session"]["delegation"]["type"], "responses");
        assert_ne!(wire["session"]["delegation"]["type"], "client");
        assert_eq!(
            wire["session"]["delegation"]["responses"]["tools"]
                .as_array()
                .expect("tools array")
                .len(),
            1
        );
        assert_eq!(
            wire["session"]["delegation"]["responses"]["tools"][0]["name"],
            GPT_LIVE_RESPONSES_BRIDGE_TOOL
        );
        assert_eq!(
            wire["session"]["delegation"]["responses"]["tools"][0]["parameters"],
            gpt_live_responses_bridge_parameters()
        );
        assert!(wire["session"]["instructions"].is_null());
    }

    #[test]
    fn opaque_transcript_and_turn_refs_preserve_exact_ids_without_debug_leakage() {
        assert_eq!(
            GptLiveTurnRole::from_provider_role("user"),
            GptLiveTurnRole::User
        );
        assert_eq!(
            GptLiveTurnRole::from_provider_role("assistant"),
            GptLiveTurnRole::Assistant
        );
        assert_eq!(
            GptLiveTurnRole::from_provider_role("unqualified-private-role"),
            GptLiveTurnRole::Unknown
        );
        let item = GptLiveTranscriptItemRef("private_input_item_id".to_string());
        let turn = GptLiveTurnRef("private_turn_id".to_string());
        assert_eq!(item.__opaque_provider_id(), "private_input_item_id");
        assert_eq!(turn.__opaque_provider_id(), "private_turn_id");
        assert!(!format!("{item:?}").contains("private_input_item_id"));
        assert!(!format!("{turn:?}").contains("private_turn_id"));
        let observations = [
            GptLiveBrokerObservation::UserTranscriptFragment {
                item,
                text: "hello".to_string(),
            },
            GptLiveBrokerObservation::TurnStarted {
                turn: turn.clone(),
                role: GptLiveTurnRole::User,
            },
            GptLiveBrokerObservation::TurnFinished {
                turn,
                role: GptLiveTurnRole::User,
                transcript: "hello".to_string(),
            },
        ];
        for observation in observations {
            let debug = format!("{observation:?}");
            assert!(!debug.contains("private_input_item_id"));
            assert!(!debug.contains("private_turn_id"));
        }
    }

    #[test]
    fn factory_requires_experimental_witness_and_chatgpt_backend() {
        let stable = GptLiveBrokerFactory::admit_target(realtime_target(
            ModelReleaseStage::Stable,
            OpenAiBackendKind::ChatGptBackend,
        ));
        assert!(matches!(
            stable,
            Err(ProviderClientError::MissingFeature(
                "openai-experimental-gpt-live-model"
            ))
        ));

        let public_backend = GptLiveBrokerFactory::admit_target(realtime_target(
            ModelReleaseStage::Experimental,
            OpenAiBackendKind::OpenAiApi,
        ));
        assert!(matches!(
            public_backend,
            Err(ProviderClientError::MissingFeature(
                "openai-experimental-gpt-live-chatgpt-backend"
            ))
        ));
    }

    #[test]
    fn factory_admits_exact_witness_and_redacts_runtime_material() {
        let factory = GptLiveBrokerFactory::from_target_with_transport(
            realtime_target(
                ModelReleaseStage::Experimental,
                OpenAiBackendKind::ChatGptBackend,
            ),
            GptLiveTransport::try_new().expect("test transport"),
        )
        .expect("experimental ChatGPT target should be admitted");
        let debug = format!("{factory:?}");
        assert!(!debug.contains("credential-secret"));
        assert!(!debug.contains("gpt-live"));
        assert!(debug.contains("registry-admitted"));
    }

    #[tokio::test]
    async fn broker_lowers_private_events_and_correlates_appends_without_exposing_provider_ids() {
        let (transport, capture, server) = local_transport().await;
        let factory = GptLiveBrokerFactory::from_target_with_transport(
            realtime_target(
                ModelReleaseStage::Experimental,
                OpenAiBackendKind::ChatGptBackend,
            ),
            transport,
        )
        .expect("admitted fixture factory");
        let config = GptLiveBrokerOpenConfig::new("PRIVATE_OFFER_SDP", "cove")
            .expect("fixture open config")
            .with_responses_session(
                GptLiveResponsesSessionConfig::try_from_catalog_model(&responses_model())
                    .expect("catalog responses profile"),
            );
        let bootstrap = factory.open(config).await.expect("broker bootstrap");
        assert_eq!(bootstrap.answer_sdp(), "v=0\r\nPRIVATE_ANSWER_SDP");
        let (_, session) = bootstrap.into_parts();

        session
            .await_ready_and_seed_session_context(Some("canonical context".to_string()))
            .await
            .expect("readiness and exact seed acknowledgement");
        assert!(matches!(
            session.next_observation().await.expect("input fragment"),
            Some(GptLiveBrokerObservation::UserTranscriptFragment { item, text })
                if item.__opaque_provider_id() == "private_input_item" && text == "input fragment"
        ));
        assert!(matches!(
            session.next_observation().await.expect("output fragment"),
            Some(GptLiveBrokerObservation::AssistantTranscriptFragment { item, text })
                if item.__opaque_provider_id() == "private_output_item" && text == "output fragment"
        ));
        assert!(matches!(
            session.next_observation().await.expect("user turn start"),
            Some(GptLiveBrokerObservation::TurnStarted { turn, role: GptLiveTurnRole::User })
                if turn.__opaque_provider_id() == "private_user_turn"
        ));
        assert!(matches!(
            session.next_observation().await.expect("turn snapshot"),
            Some(GptLiveBrokerObservation::TurnSnapshotDelta { turn, delta })
                if turn.__opaque_provider_id() == "private_user_turn"
                    && delta == "unqualified snapshot"
        ));
        assert!(matches!(
            session.next_observation().await.expect("user turn final"),
            Some(GptLiveBrokerObservation::TurnFinished {
                turn,
                role: GptLiveTurnRole::User,
                transcript,
            }) if turn.__opaque_provider_id() == "private_user_turn"
                && transcript == "authoritative user final"
        ));
        assert!(matches!(
            session.next_observation().await.expect("assistant turn start"),
            Some(GptLiveBrokerObservation::TurnStarted {
                turn,
                role: GptLiveTurnRole::Assistant,
            }) if turn.__opaque_provider_id() == "private_assistant_turn"
        ));
        assert!(matches!(
            session.next_observation().await.expect("assistant turn final"),
            Some(GptLiveBrokerObservation::TurnFinished {
                turn,
                role: GptLiveTurnRole::Assistant,
                transcript,
            }) if turn.__opaque_provider_id() == "private_assistant_turn"
                && transcript == "authoritative assistant final"
        ));
        let delegation = match session.next_observation().await.expect("delegation") {
            Some(GptLiveBrokerObservation::DelegationActionableInputUnsupported { delegation }) => {
                delegation
            }
            other => panic!("expected fail-closed delegation observation, got {other:?}"),
        };
        let delegation_debug = format!("{delegation:?}");
        assert!(!delegation_debug.contains("private_delegation_id"));

        let delegation_token = session
            .append_delegation_context(&delegation, "executor result")
            .await
            .expect("send delegation context");
        assert_eq!(
            session.next_observation().await.expect("delegation ack"),
            Some(
                GptLiveBrokerObservation::DelegationContextAppendAcknowledged {
                    token: delegation_token
                }
            )
        );

        let capture = capture.lock().expect("capture lock");
        let call_body = capture.call_body.as_ref().expect("captured call body");
        assert_eq!(call_body["sdp"], "PRIVATE_OFFER_SDP");
        assert_eq!(call_body["session"]["delegation"]["type"], "responses");
        assert_ne!(call_body["session"]["delegation"]["type"], "client");
        assert_eq!(capture.client_events.len(), 2);
        assert_eq!(capture.client_events[0]["type"], "session.context.append");
        assert_eq!(capture.client_events[0]["channel"], "commentary");
        assert_eq!(
            capture.client_events[1]["type"],
            "delegation.context.append"
        );
        assert_eq!(capture.client_events[1]["channel"], "commentary");
        drop(capture);
        server.abort();
    }

    #[tokio::test]
    async fn initial_seed_failure_closes_without_retrying_context_append() {
        let (transport, capture, server) = local_transport_rejecting_seed().await;
        let factory = GptLiveBrokerFactory::from_target_with_transport(
            realtime_target(
                ModelReleaseStage::Experimental,
                OpenAiBackendKind::ChatGptBackend,
            ),
            transport,
        )
        .expect("admitted fixture factory");
        let bootstrap = factory
            .open(
                GptLiveBrokerOpenConfig::new("PRIVATE_OFFER_SDP", "cove")
                    .expect("fixture open config"),
            )
            .await
            .expect("broker bootstrap");
        let (_, session) = bootstrap.into_parts();
        assert!(matches!(
            session
                .await_ready_and_seed_session_context(Some("canonical context".to_string()))
                .await,
            Err(GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol
            })
        ));
        let capture = capture.lock().expect("capture lock");
        assert_eq!(capture.client_events.len(), 1);
        assert_eq!(capture.client_events[0]["type"], "session.context.append");
        assert_eq!(capture.client_events[0]["channel"], "commentary");
        drop(capture);
        server.abort();
    }
}
