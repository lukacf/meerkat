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
    CallSession, ClientDelegation, ClientEvent, ContextChannel, CreateCallRequest, Delegation,
    DelegationContextAppend, ExtraFields, FunctionTool, GptLiveCredentials, GptLiveTransport,
    InputTextContent, ResponsesConfig, ResponsesDelegation, ServerEvent, SessionAudio,
    SessionAudioOutput, SessionContextAppend, SidebandHeaders, SidebandReceiver, SidebandSender,
    TransportError,
};
use std::collections::{HashMap, HashSet, VecDeque};
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
/// The caller supplies only a registry-minted model witness. Tool identity,
/// description, and arguments schema are fixed by this profile; no caller or
/// operator prose can enter the provider session through it.
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
                instructions: None,
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
            .field("tool", &GPT_LIVE_RESPONSES_BRIDGE_TOOL)
            .finish()
    }
}

/// Provider-owned mechanical configuration for one browser WebRTC bootstrap.
#[derive(Clone)]
pub struct GptLiveBrokerOpenConfig {
    offer_sdp: String,
    voice: String,
    delegation: Option<GptLiveBrokerDelegationConfig>,
    session_instructions: Option<String>,
}

#[derive(Clone)]
enum GptLiveBrokerDelegationConfig {
    Client,
    Responses(GptLiveResponsesSessionConfig),
}

impl GptLiveBrokerDelegationConfig {
    fn mode(&self) -> GptLiveBrokerDelegationMode {
        match self {
            Self::Client => GptLiveBrokerDelegationMode::Client,
            Self::Responses(_) => GptLiveBrokerDelegationMode::Responses,
        }
    }

    fn into_delegation(self) -> Delegation {
        match self {
            Self::Client => Delegation::Client(ClientDelegation::default()),
            Self::Responses(responses) => responses.into_delegation(),
        }
    }
}

impl std::fmt::Debug for GptLiveBrokerDelegationConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client => formatter.write_str("Client(<platform-owned>)"),
            Self::Responses(responses) => responses.fmt(formatter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GptLiveBrokerDelegationMode {
    None,
    Client,
    Responses,
}

impl std::fmt::Debug for GptLiveBrokerOpenConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GptLiveBrokerOpenConfig")
            .field("offer_sdp", &"<redacted>")
            .field("voice", &"<redacted>")
            .field("delegation", &self.delegation)
            .field(
                "session_instructions",
                &self
                    .session_instructions
                    .as_ref()
                    .map(|_| "<catalog-bound>"),
            )
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
            delegation: None,
            session_instructions: None,
        })
    }

    /// Select the fixed platform-owned client-context delegation mode.
    ///
    /// This adds no consumer tools or provider-side Responses configuration.
    /// The caller receives only typed client delegation joins and may return
    /// context through `delegation.context.append`.
    #[must_use]
    pub fn with_client_delegation(mut self) -> Self {
        self.delegation = Some(GptLiveBrokerDelegationConfig::Client);
        self
    }

    /// Select the catalog-bound Responses function bridge.
    ///
    /// Client-context is a separate explicit configuration and cannot be
    /// silently substituted for this mode.
    #[must_use]
    pub fn with_responses_session(mut self, responses: GptLiveResponsesSessionConfig) -> Self {
        self.delegation = Some(GptLiveBrokerDelegationConfig::Responses(responses));
        self
    }

    fn delegation_mode(&self) -> GptLiveBrokerDelegationMode {
        self.delegation
            .as_ref()
            .map_or(GptLiveBrokerDelegationMode::None, |delegation| {
                delegation.mode()
            })
    }

    /// Lower host-catalog guidance into the verified top-level GPT Live call
    /// session field. The experimental Meerkat admission witness is the only
    /// shipping caller of this seam; raw live/open prose never reaches it.
    #[must_use]
    pub fn with_session_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.session_instructions = Some(instructions.into());
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

/// Opaque provider handoff identity retained only for exact client joins.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GptLiveHandoffRef(String);

impl GptLiveHandoffRef {
    #[doc(hidden)]
    #[must_use]
    pub fn __opaque_provider_id(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GptLiveHandoffRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveHandoffRef(<redacted>)")
    }
}

/// Qualified target carried by a joined client delegation observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptLiveDelegationTarget {
    Client,
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
    /// Exact client-targeted delegation joined to its final user turn.
    ///
    /// This is provider evidence only. It does not itself authorize executor
    /// work or establish canonical transcript commitment.
    ClientDelegationFinal {
        delegation: GptLiveDelegationRef,
        target: GptLiveDelegationTarget,
        handoff: GptLiveHandoffRef,
        turn: GptLiveTurnRef,
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
            Self::ClientDelegationFinal { .. } => "client_delegation_final",
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
        let delegation_mode = config.delegation_mode();
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
                    delegation_mode,
                    pending_client_delegations: HashMap::new(),
                    seen_delegation_ids: HashSet::new(),
                    seen_handoff_ids: HashSet::new(),
                    seen_user_turn_ids: HashSet::new(),
                    queued_observations: VecDeque::new(),
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
                    .delegation
                    .map(GptLiveBrokerDelegationConfig::into_delegation),
                instructions: config.session_instructions,
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
    delegation_mode: GptLiveBrokerDelegationMode,
    pending_client_delegations: HashMap<String, PendingClientDelegation>,
    seen_delegation_ids: HashSet<String>,
    seen_handoff_ids: HashSet<String>,
    seen_user_turn_ids: HashSet<String>,
    queued_observations: VecDeque<GptLiveBrokerObservation>,
}

struct PendingClientDelegation {
    delegation: GptLiveDelegationRef,
    handoff: GptLiveHandoffRef,
    turn: GptLiveTurnRef,
}

impl std::fmt::Debug for GptLiveBrokerSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveBrokerSession(<connected>)")
    }
}

impl GptLiveBrokerSession {
    /// Seed one ordered canonical commentary envelope after the answer has
    /// been delivered and the private sideband has connected. GPT Live emits
    /// `session.started` on the browser data channel, not on this sideband, so
    /// the exact context-append acknowledgement is the server-side readiness
    /// evidence when a seed exists. Valid observations that race ahead of the
    /// acknowledgement are preserved in order. Any missing, ambiguous, or
    /// mismatched acknowledgement closes the partial session; this method
    /// never retries.
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
                    Some(GptLiveBrokerObservation::SessionReady) => {
                        // The provider-neutral host emits one readiness fact
                        // after this exact seed resolver succeeds. Do not
                        // replay a redundant private-protocol observation.
                    }
                    Some(GptLiveBrokerObservation::UnsupportedPrivateEvent) => {
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
    /// In client mode, delegation and user-final events are joined inside this
    /// serialized receive owner. A matched client delegation is the sole
    /// terminal observation for that user turn; an ordinary `TurnFinished` is
    /// emitted only when no delegation was pending at the provider-final
    /// boundary. This is provider evidence only and grants no executor
    /// authority.
    pub async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
        let mut receiver = self.receiver.lock().await;
        loop {
            if let Some(observation) = self.state.lock().await.queued_observations.pop_front() {
                return Ok(Some(observation));
            }
            let Some(event) = receiver.next_event().await? else {
                return Ok(None);
            };
            self.state.lock().await.apply_event(event)?;
        }
    }

    /// Close the private sideband without exposing its wire identity.
    pub async fn close(&self) -> Result<(), GptLiveBrokerError> {
        self.sender.close().await.map_err(Into::into)
    }
}

impl GptLiveBrokerSessionState {
    const MAX_CLIENT_JOIN_IDENTITIES: usize = 4096;

    fn allocate_append_token(&mut self) -> GptLiveAppendToken {
        let token = GptLiveAppendToken(self.next_append_token);
        self.next_append_token = self.next_append_token.saturating_add(1);
        token
    }

    fn apply_event(&mut self, event: ServerEvent) -> Result<(), GptLiveBrokerError> {
        match event {
            ServerEvent::SessionStarted(_) => self
                .queued_observations
                .push_back(GptLiveBrokerObservation::SessionReady),
            ServerEvent::SessionUsageUpdated(_) => {
                // Observed provider accounting telemetry. It carries no
                // conversational, transcript, delegation, or effect
                // authority and is intentionally not projected upward.
            }
            ServerEvent::SessionContextAppended(_) => {
                let token = self
                    .pending_session_append
                    .take()
                    .ok_or_else(protocol_error)?;
                self.queued_observations.push_back(
                    GptLiveBrokerObservation::SessionContextAppendAcknowledged { token },
                );
            }
            ServerEvent::InputTranscriptAdded(event) => self.queued_observations.push_back(
                GptLiveBrokerObservation::UserTranscriptFragment {
                    item: GptLiveTranscriptItemRef(event.item.id),
                    text: event.item.text,
                },
            ),
            ServerEvent::OutputTranscriptAdded(event) => self.queued_observations.push_back(
                GptLiveBrokerObservation::AssistantTranscriptFragment {
                    item: GptLiveTranscriptItemRef(event.item.id),
                    text: event.item.text,
                },
            ),
            ServerEvent::TurnCreated(event) => {
                let role = GptLiveTurnRole::from_provider_role(&event.turn.role);
                tracing::debug!(?role, "lowered GPT Live turn start");
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::TurnStarted {
                        turn: GptLiveTurnRef(event.turn.id),
                        role,
                    });
            }
            ServerEvent::TurnDelta(event) => {
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::TurnSnapshotDelta {
                        turn: GptLiveTurnRef(event.turn_id),
                        delta: event.delta,
                    });
            }
            ServerEvent::TurnDone(event) => {
                let role = GptLiveTurnRole::from_provider_role(&event.turn.role);
                tracing::debug!(?role, "lowered GPT Live turn finish");
                let turn = GptLiveTurnRef(event.turn.id);
                let transcript = event.turn.transcript;
                if self.delegation_mode == GptLiveBrokerDelegationMode::Client
                    && role == GptLiveTurnRole::User
                {
                    if let Some(pending) = self.record_client_user_final(&turn)? {
                        self.push_client_join(pending, turn, transcript);
                    } else {
                        self.queued_observations.push_back(
                            GptLiveBrokerObservation::TurnFinished {
                                turn,
                                role,
                                transcript,
                            },
                        );
                    }
                } else {
                    self.queued_observations
                        .push_back(GptLiveBrokerObservation::TurnFinished {
                            turn,
                            role,
                            transcript,
                        });
                }
            }
            ServerEvent::DelegationCreated(event) => {
                if self.delegation_mode == GptLiveBrokerDelegationMode::Client {
                    self.record_client_delegation(event.item)?;
                } else {
                    self.queued_observations.push_back(
                        GptLiveBrokerObservation::DelegationActionableInputUnsupported {
                            delegation: GptLiveDelegationRef(event.item.id),
                        },
                    );
                }
            }
            ServerEvent::DelegationContextAppended(event) => {
                let (delegation, token) = self
                    .pending_delegation_append
                    .take()
                    .ok_or_else(protocol_error)?;
                if delegation.0 != event.delegation_item_id {
                    return Err(protocol_error());
                }
                self.queued_observations.push_back(
                    GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token },
                );
            }
            ServerEvent::Unknown(event) => {
                let summary = summarize_unknown_private_event(&event);
                tracing::warn!(
                    provider_event_class = "unknown",
                    error_class = summary.error_class,
                    top_level_field_count = summary.top_level_field_count,
                    normalized_json_bytes = summary.normalized_json_bytes,
                    message_bytes = summary.message_bytes,
                    "experimental GPT Live received an unsupported private sideband event"
                );
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedPrivateEvent);
            }
        }
        Ok(())
    }

    fn record_client_delegation(
        &mut self,
        item: oai_rt_rs::experimental::gpt_live::DelegationItem,
    ) -> Result<(), GptLiveBrokerError> {
        if item.item_type != "delegation"
            || item.target != "client"
            || item.id.trim().is_empty()
            || item.handoff_id.trim().is_empty()
            || item.user_bidi_turn_id.trim().is_empty()
            || self.seen_delegation_ids.contains(&item.id)
            || self.seen_handoff_ids.contains(&item.handoff_id)
            || self
                .pending_client_delegations
                .contains_key(&item.user_bidi_turn_id)
            || self.seen_user_turn_ids.contains(&item.user_bidi_turn_id)
            || self.seen_delegation_ids.len() >= Self::MAX_CLIENT_JOIN_IDENTITIES
            || self.seen_handoff_ids.len() >= Self::MAX_CLIENT_JOIN_IDENTITIES
        {
            return Err(protocol_error());
        }

        let turn_id = item.user_bidi_turn_id;
        let pending = PendingClientDelegation {
            delegation: GptLiveDelegationRef(item.id.clone()),
            handoff: GptLiveHandoffRef(item.handoff_id.clone()),
            turn: GptLiveTurnRef(turn_id.clone()),
        };
        self.seen_delegation_ids.insert(item.id);
        self.seen_handoff_ids.insert(item.handoff_id);
        self.pending_client_delegations.insert(turn_id, pending);
        Ok(())
    }

    fn record_client_user_final(
        &mut self,
        turn: &GptLiveTurnRef,
    ) -> Result<Option<PendingClientDelegation>, GptLiveBrokerError> {
        if turn.0.trim().is_empty()
            || self.seen_user_turn_ids.contains(&turn.0)
            || self.seen_user_turn_ids.len() >= Self::MAX_CLIENT_JOIN_IDENTITIES
        {
            return Err(protocol_error());
        }
        self.seen_user_turn_ids.insert(turn.0.clone());
        Ok(self.pending_client_delegations.remove(&turn.0))
    }

    fn push_client_join(
        &mut self,
        pending: PendingClientDelegation,
        turn: GptLiveTurnRef,
        transcript: String,
    ) {
        debug_assert!(pending.turn == turn);
        self.queued_observations
            .push_back(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation: pending.delegation,
                target: GptLiveDelegationTarget::Client,
                handoff: pending.handoff,
                turn,
                transcript,
            });
    }
}

fn protocol_error() -> GptLiveBrokerError {
    GptLiveBrokerError::Transport {
        class: GptLiveBrokerTerminalClass::Protocol,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnknownPrivateEventSummary {
    error_class: &'static str,
    top_level_field_count: usize,
    normalized_json_bytes: usize,
    message_bytes: usize,
}

fn summarize_unknown_private_event(
    event: &oai_rt_rs::experimental::gpt_live::UnknownEvent,
) -> UnknownPrivateEventSummary {
    let message = event
        .raw()
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let error_class = if message.contains("maximum")
        || message.contains("too long")
        || message.contains("exceed")
    {
        "size_limit"
    } else if message.contains("Unknown parameter") {
        "unknown_parameter"
    } else if message.contains("Missing required parameter") {
        "missing_parameter"
    } else if message.contains("Invalid") || message.contains("invalid") {
        "invalid_parameter"
    } else if event.kind() == "error" {
        "other_provider_error"
    } else {
        "unsupported_event"
    };
    UnknownPrivateEventSummary {
        error_class,
        top_level_field_count: event.raw().as_object().map_or(0, serde_json::Map::len),
        normalized_json_bytes: serde_json::to_vec(event.raw()).map_or(0, |bytes| bytes.len()),
        message_bytes: message.len(),
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

    fn broker_state(mode: GptLiveBrokerDelegationMode) -> GptLiveBrokerSessionState {
        GptLiveBrokerSessionState {
            next_append_token: 1,
            pending_session_append: None,
            pending_delegation_append: None,
            delegation_mode: mode,
            pending_client_delegations: HashMap::new(),
            seen_delegation_ids: HashSet::new(),
            seen_handoff_ids: HashSet::new(),
            seen_user_turn_ids: HashSet::new(),
            queued_observations: VecDeque::new(),
        }
    }

    fn captured_client_delegation(turn_id: &str) -> ServerEvent {
        oai_rt_rs::experimental::gpt_live::decode_server_event(
            &json!({
                "type": "delegation.created",
                "offset_ms": 1,
                "item": {
                    "id": "item_EGKFFURbWV7QZwDEWG06L",
                    "type": "delegation",
                    "target": "client",
                    "handoff_id": "handoff_1",
                    "user_bidi_turn_id": turn_id,
                    "content": []
                }
            })
            .to_string(),
        )
        .expect("captured client delegation event")
    }

    fn captured_user_turn_done(turn_id: &str, transcript: &str) -> ServerEvent {
        oai_rt_rs::experimental::gpt_live::decode_server_event(
            &json!({
                "type": "turn.done",
                "turn": {
                    "id": turn_id,
                    "role": "user",
                    "start_ms": 6,
                    "end_ms": 8,
                    "transcript": transcript
                }
            })
            .to_string(),
        )
        .expect("captured user turn final")
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
    fn unknown_private_event_summary_contains_only_fixed_classes_and_counts() {
        let event = oai_rt_rs::experimental::gpt_live::decode_server_event(
            &json!({
                "type": "FIXTURE_PRIVATE_UNKNOWN_KIND",
                "error": {
                    "message": "Invalid FIXTURE_PRIVATE_MESSAGE_SECRET"
                },
                "secret": "FIXTURE_PRIVATE_PAYLOAD_SECRET"
            })
            .to_string(),
        )
        .expect("unknown fixture event");
        let ServerEvent::Unknown(event) = event else {
            panic!("fixture must remain unknown");
        };

        let summary = summarize_unknown_private_event(&event);
        assert_eq!(summary.error_class, "invalid_parameter");
        assert_eq!(summary.top_level_field_count, 3);
        assert!(summary.normalized_json_bytes > 0);
        assert!(summary.message_bytes > 0);
        let diagnostics = format!("{summary:?}");
        assert!(!diagnostics.contains("FIXTURE_PRIVATE_UNKNOWN_KIND"));
        assert!(!diagnostics.contains("FIXTURE_PRIVATE_MESSAGE_SECRET"));
        assert!(!diagnostics.contains("FIXTURE_PRIVATE_PAYLOAD_SECRET"));
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
        if let Some(Ok(AxumMessage::Text(text))) = socket.recv().await {
            capture
                .lock()
                .expect("capture lock")
                .client_events
                .push(serde_json::from_str(&text).expect("session append JSON"));
            socket
                .send(AxumMessage::Text(
                    json!({
                        "type": "input_transcript.added",
                        "start_ms": 0,
                        "end_ms": 1,
                        "item": { "id": "private_input_item", "type": "transcript", "text": "input fragment" }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .expect("interleave transcript before session append acknowledgement");
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
                "type": "delegation.created",
                "offset_ms": 1,
                "item": {
                    "id": "private_delegation_id",
                    "type": "delegation",
                    "target": "client",
                    "handoff_id": "private_handoff_id",
                    "user_bidi_turn_id": "private_user_turn",
                    "content": []
                }
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
        assert!(debug.contains(GPT_LIVE_RESPONSES_BRIDGE_TOOL));
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
        assert!(
            wire["session"]["delegation"]["responses"]
                .get("instructions")
                .is_none()
        );
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
    fn client_context_open_config_produces_only_fixed_client_delegation() {
        let target = realtime_target(
            ModelReleaseStage::Experimental,
            OpenAiBackendKind::ChatGptBackend,
        );
        let factory = GptLiveBrokerFactory::from_target_with_transport(
            target,
            GptLiveTransport::try_new().expect("test transport"),
        )
        .expect("experimental ChatGPT target");
        let request = factory.call_request(
            GptLiveBrokerOpenConfig::new("PRIVATE_OFFER_SDP", "cove")
                .expect("open config")
                .with_client_delegation(),
        );
        let wire = serde_json::to_value(request).expect("serialize call request");

        assert_eq!(wire["session"]["delegation"]["type"], "client");
        assert!(wire["session"]["delegation"].get("responses").is_none());
        assert!(wire["session"]["delegation"].get("tools").is_none());
    }

    #[test]
    fn client_delegation_then_user_final_emits_only_exact_join() {
        let turn_id = "turn_EGKFBvJNmTZWroiawtuhO";
        let mut state = broker_state(GptLiveBrokerDelegationMode::Client);
        state
            .apply_event(captured_client_delegation(turn_id))
            .expect("retain client delegation");
        assert!(state.queued_observations.is_empty());

        state
            .apply_event(captured_user_turn_done(turn_id, "authoritative final"))
            .expect("join final user turn");
        assert!(matches!(
            state.queued_observations.pop_front(),
            Some(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation,
                target: GptLiveDelegationTarget::Client,
                handoff,
                turn,
                transcript,
            }) if delegation.__opaque_provider_id() == "item_EGKFFURbWV7QZwDEWG06L"
                && handoff.__opaque_provider_id() == "handoff_1"
                && turn.__opaque_provider_id() == turn_id
                && transcript == "authoritative final"
        ));
        assert!(state.queued_observations.is_empty());
    }

    #[test]
    fn client_delegation_after_terminal_user_turn_fails_closed() {
        let turn_id = "turn_EGKFBvJNmTZWroiawtuhO";
        let mut state = broker_state(GptLiveBrokerDelegationMode::Client);
        state
            .apply_event(captured_user_turn_done(turn_id, "authoritative final"))
            .expect("retain final user turn");
        assert!(matches!(
            state.queued_observations.pop_front(),
            Some(GptLiveBrokerObservation::TurnFinished { .. })
        ));

        assert_protocol_error(
            state
                .apply_event(captured_client_delegation(turn_id))
                .expect_err("late delegation must not duplicate a terminal user turn"),
        );
        assert!(state.queued_observations.is_empty());
    }

    #[test]
    fn client_delegation_duplicate_and_wrong_target_fail_closed() {
        let turn_id = "turn_EGKFBvJNmTZWroiawtuhO";
        let mut duplicate = broker_state(GptLiveBrokerDelegationMode::Client);
        duplicate
            .apply_event(captured_client_delegation(turn_id))
            .expect("first delegation");
        assert_protocol_error(
            duplicate
                .apply_event(captured_client_delegation(turn_id))
                .expect_err("duplicate delegation must fail"),
        );

        let mut wrong_target = broker_state(GptLiveBrokerDelegationMode::Client);
        let event = oai_rt_rs::experimental::gpt_live::decode_server_event(
            &json!({
                "type": "delegation.created",
                "offset_ms": 1,
                "item": {
                    "id": "item_EGKFFURbWV7QZwDEWG06L",
                    "type": "delegation",
                    "target": "responses",
                    "handoff_id": "handoff_1",
                    "user_bidi_turn_id": turn_id,
                    "content": []
                }
            })
            .to_string(),
        )
        .expect("wrong-target delegation event");
        assert_protocol_error(
            wrong_target
                .apply_event(event)
                .expect_err("wrong target must fail"),
        );
        assert!(wrong_target.pending_client_delegations.is_empty());
    }

    #[test]
    fn client_delegation_never_joins_a_mismatched_user_turn() {
        let expected_turn = "turn_EGKFBvJNmTZWroiawtuhO";
        let other_turn = "turn_other";
        let mut state = broker_state(GptLiveBrokerDelegationMode::Client);
        state
            .apply_event(captured_client_delegation(expected_turn))
            .expect("retain delegation");
        state
            .apply_event(captured_user_turn_done(other_turn, "other final"))
            .expect("retain unrelated final");
        assert!(matches!(
            state.queued_observations.pop_front(),
            Some(GptLiveBrokerObservation::TurnFinished { turn, .. })
                if turn.__opaque_provider_id() == other_turn
        ));
        assert!(state.queued_observations.is_empty());

        state
            .apply_event(captured_user_turn_done(expected_turn, "matching final"))
            .expect("join matching final");
        assert!(matches!(
            state.queued_observations.pop_front(),
            Some(GptLiveBrokerObservation::ClientDelegationFinal { turn, transcript, .. })
                if turn.__opaque_provider_id() == expected_turn
                    && transcript == "matching final"
        ));
    }

    #[test]
    fn catalog_session_instructions_lower_only_to_top_level_call_session() {
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
                .with_responses_session(responses)
                .with_session_instructions("Converse as the selected voice embodiment."),
        );
        let wire = serde_json::to_value(request).expect("serialize call request");

        assert_eq!(
            wire["session"]["instructions"],
            "Converse as the selected voice embodiment."
        );
        assert!(
            wire["session"]["delegation"]["responses"]
                .get("instructions")
                .is_none(),
            "top-level channel guidance must not populate delegation.responses.instructions"
        );
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
            .with_client_delegation();
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
        let delegation = match session.next_observation().await.expect("joined delegation") {
            Some(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation,
                target: GptLiveDelegationTarget::Client,
                handoff,
                turn,
                transcript,
            }) if handoff.__opaque_provider_id() == "private_handoff_id"
                && turn.__opaque_provider_id() == "private_user_turn"
                && transcript == "authoritative user final" =>
            {
                delegation
            }
            other => panic!("expected exact joined delegation, got {other:?}"),
        };
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
        assert_eq!(call_body["session"]["delegation"]["type"], "client");
        assert!(
            call_body["session"]["delegation"]
                .get("responses")
                .is_none()
        );
        assert!(call_body["session"]["delegation"].get("tools").is_none());
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
