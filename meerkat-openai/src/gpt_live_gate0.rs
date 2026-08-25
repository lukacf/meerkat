//! Non-shipping direct-protocol Gate0 candidate broker.
//!
//! This module is structurally separate from the qualified experimental
//! factory. Its target is not convertible to `AdmittedExperimentalRealtimeTarget`
//! and its factory never accepts that shipping carrier.

use std::collections::HashMap;

use meerkat_core::{AuthBindingUseWitness, ProviderAuthMetadata};
use meerkat_llm_core::provider_runtime::{NormalizedBackendKind, ResolvedRealtimeTarget};
use oai_rt_rs::experimental::gpt_live::{
    CallSession, ClientEvent, CreateCallRequest, Delegation, DelegationFunctionCallOutput,
    EventCarrier, ExtraFields, FunctionCallId, FunctionCallOutput, FunctionTool,
    GptLiveCredentials, GptLiveTransport, MAX_FUNCTION_OUTPUT_BYTES, MAX_RAW_JSON_EVENT_BYTES,
    ReceivedServerEvent, ResponsesConfig, ResponsesDelegation, ServerEvent, SessionAudio,
    SessionAudioOutput, SidebandHeaders, SidebandReceiver, SidebandSender, TransportError,
};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::OpenAiBackendKind;

/// Fixed declaration emitted by sanitized evidence. It is not a protocol
/// digest and cannot satisfy any qualified-build admission predicate.
pub const GATE0_CANDIDATE_CONTRACT: &str = "unqualified-direct-gate0-v1";

pub const GATE0_RESPONSES_BRIDGE_TOOL: &str = "invoke_meerkat";

const GATE0_RESPONSES_BRIDGE_DESCRIPTION: &str =
    "Delegate this request to the channel-bound Meerkat agent.";

fn strict_bridge_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "request": { "type": "string" }
        },
        "required": ["request"],
        "additionalProperties": false
    })
}

/// Explicit status of this candidate-only transport. This value cannot be
/// converted into any shipping qualification or admission witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate0CandidateQualification {
    LiveUnqualified,
}

/// Provider-owned Responses bridge configuration for one unqualified Gate0
/// run. Its only tool is the strict authority-free `invoke_meerkat` bridge.
#[derive(Clone)]
pub struct Gate0ResponsesCandidateConfig {
    responses: ResponsesConfig,
}

impl Gate0ResponsesCandidateConfig {
    pub fn try_new(bridge_model: impl Into<String>) -> Result<Self, Gate0CandidateError> {
        let bridge_model = bridge_model.into();
        if bridge_model.trim().is_empty() {
            return Err(Gate0CandidateError::InvalidResponsesConfiguration);
        }
        Ok(Self {
            responses: ResponsesConfig {
                model: bridge_model,
                instructions: None,
                tools: vec![FunctionTool::new(
                    GATE0_RESPONSES_BRIDGE_TOOL,
                    GATE0_RESPONSES_BRIDGE_DESCRIPTION,
                    strict_bridge_parameters(),
                    ExtraFields::new(),
                )],
                extra: ExtraFields::new(),
            },
        })
    }

    #[must_use]
    pub const fn qualification(&self) -> Gate0CandidateQualification {
        Gate0CandidateQualification::LiveUnqualified
    }

    #[must_use]
    pub const fn delegation_type(&self) -> &'static str {
        "responses"
    }

    #[must_use]
    pub fn bridge_model_present(&self) -> bool {
        !self.responses.model.trim().is_empty()
    }

    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.responses.tools.len()
    }

    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        self.responses.tools.first().map(|tool| tool.name.as_str())
    }

    #[must_use]
    pub fn strict_arguments_schema(&self) -> bool {
        self.responses.tools.len() == 1
            && self.responses.tools[0].parameters == strict_bridge_parameters()
    }

    /// Proves that the actual candidate delegation serializes to the exact
    /// model/single-tool allow-list, with no extra authority or
    /// provider fields at any level.
    #[must_use]
    pub fn exact_authority_free_shape(&self) -> bool {
        let expected = serde_json::json!({
            "type": "responses",
            "responses": {
                "model": &self.responses.model,
                "tools": [{
                    "type": "function",
                    "name": GATE0_RESPONSES_BRIDGE_TOOL,
                    "description": GATE0_RESPONSES_BRIDGE_DESCRIPTION,
                    "parameters": strict_bridge_parameters()
                }]
            }
        });
        serde_json::to_value(self.delegation()).is_ok_and(|actual| actual == expected)
    }

    fn delegation(&self) -> Delegation {
        Delegation::Responses(ResponsesDelegation::new(
            self.responses.clone(),
            ExtraFields::new(),
        ))
    }
}

impl std::fmt::Debug for Gate0ResponsesCandidateConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate0ResponsesCandidateConfig")
            .field("bridge_model", &"<redacted>")
            .field("tool_count", &self.tool_count())
            .field("qualification", &"live-unqualified")
            .finish()
    }
}

/// Exact registry target plus explicit binding-use authorization for one
/// unqualified Gate0 run.
pub struct Gate0CandidateRealtimeTarget {
    target: ResolvedRealtimeTarget,
    _binding_use: AuthBindingUseWitness,
}

impl std::fmt::Debug for Gate0CandidateRealtimeTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate0CandidateRealtimeTarget")
            .field("provider", &self.target.identity().provider)
            .field("model", &"<registry-validated>")
            .field("binding", &"<authorized-redacted>")
            .field("qualification", &"unqualified-candidate")
            .finish()
    }
}

impl Gate0CandidateRealtimeTarget {
    pub fn try_new(
        target: ResolvedRealtimeTarget,
        binding_use: AuthBindingUseWitness,
    ) -> Result<Self, Gate0CandidateError> {
        if target.identity().auth_binding.as_ref() != Some(binding_use.auth_binding()) {
            return Err(Gate0CandidateError::BindingUseMismatch);
        }
        let profile = target.profile().profile();
        if profile.release_stage != meerkat_core::ModelReleaseStage::Experimental
            || !profile.realtime
        {
            return Err(Gate0CandidateError::TargetRejected);
        }
        Ok(Self {
            target,
            _binding_use: binding_use,
        })
    }

    fn into_target(self) -> ResolvedRealtimeTarget {
        self.target
    }
}

pub struct Gate0CandidateBrokerFactory {
    model: String,
    responses: Gate0ResponsesCandidateConfig,
    transport: GptLiveTransport,
    credentials: GptLiveCredentials,
}

impl std::fmt::Debug for Gate0CandidateBrokerFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate0CandidateBrokerFactory")
            .field("model", &"<registry-validated>")
            .field("responses", &self.responses)
            .field("transport", &"<private-transport>")
            .field("credentials", &"<redacted>")
            .field("qualification", &"unqualified-candidate")
            .finish()
    }
}

impl Gate0CandidateBrokerFactory {
    pub fn try_from_candidate(
        candidate: Gate0CandidateRealtimeTarget,
        responses: Gate0ResponsesCandidateConfig,
    ) -> Result<Self, Gate0CandidateError> {
        let target = candidate.into_target();
        let (identity, _, connection) = target.into_parts();
        if !matches!(
            connection.backend,
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend)
        ) || connection.resolved_authorizer().is_some()
        {
            return Err(Gate0CandidateError::TargetRejected);
        }
        let bearer_token = connection
            .resolved_secret()
            .ok_or(Gate0CandidateError::MissingCredential)?;
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
        let transport =
            GptLiveTransport::try_new().map_err(|_| Gate0CandidateError::TransportConfiguration)?;
        Ok(Self {
            model: identity.model,
            responses,
            transport,
            credentials,
        })
    }

    #[must_use]
    pub const fn qualification(&self) -> Gate0CandidateQualification {
        Gate0CandidateQualification::LiveUnqualified
    }

    #[must_use]
    pub const fn responses_config(&self) -> &Gate0ResponsesCandidateConfig {
        &self.responses
    }

    pub async fn answer(
        &self,
        offer_sdp: String,
        voice: String,
    ) -> Result<Gate0CandidateBootstrap, Gate0CandidateError> {
        if offer_sdp.trim().is_empty() || voice.trim().is_empty() {
            return Err(Gate0CandidateError::InvalidOpen);
        }
        let request = CreateCallRequest {
            sdp: offer_sdp,
            session: CallSession {
                model: self.model.clone(),
                audio: SessionAudio {
                    output: SessionAudioOutput {
                        voice,
                        extra: ExtraFields::new(),
                    },
                    extra: ExtraFields::new(),
                },
                delegation: Some(self.responses.delegation()),
                instructions: None,
                extra: ExtraFields::new(),
            },
        };
        let created = self
            .transport
            .create_call(&request, &self.credentials)
            .await
            .map_err(Gate0CandidateError::from)?;
        let sideband = self
            .transport
            .connect_sideband(&created.call_id, &self.credentials)
            .await
            .map_err(Gate0CandidateError::from)?;
        let (sender, receiver) = sideband.split();
        Ok(Gate0CandidateBootstrap {
            answer_sdp: created.answer_sdp,
            sideband: Gate0CandidateSideband {
                sender,
                receiver: Mutex::new(receiver),
                correlations: Mutex::new(Gate0CandidateCorrelations::default()),
            },
        })
    }
}

pub struct Gate0CandidateBootstrap {
    answer_sdp: String,
    sideband: Gate0CandidateSideband,
}

impl std::fmt::Debug for Gate0CandidateBootstrap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate0CandidateBootstrap")
            .field("answer_sdp", &"<redacted>")
            .field("sideband", &self.sideband)
            .finish()
    }
}

impl Gate0CandidateBootstrap {
    pub fn into_parts(self) -> (String, Gate0CandidateSideband) {
        (self.answer_sdp, self.sideband)
    }
}

/// Opaque provider-owned sideband used only to prove authenticated attachment,
/// readiness, full-duplex lifetime, and clean close.
pub struct Gate0CandidateSideband {
    sender: SidebandSender,
    receiver: Mutex<SidebandReceiver>,
    correlations: Mutex<Gate0CandidateCorrelations>,
}

impl std::fmt::Debug for Gate0CandidateSideband {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateSideband(<connected-redacted>)")
    }
}

impl Gate0CandidateSideband {
    /// Read the next private-protocol event and lower it into a redacted,
    /// candidate-only observation. Provider identifiers remain private while
    /// exact turn/delegation joins survive through adapter-local refs.
    pub async fn next_observation(
        &self,
    ) -> Result<Option<Gate0CandidateObservation>, Gate0CandidateError> {
        let observation = self.receiver.lock().await.next_observation().await?;
        let Some(observation) = observation else {
            return Ok(None);
        };
        let mut correlations = self.correlations.lock().await;
        Ok(Some(correlations.lower(observation)?))
    }

    /// Write one exact Responses function-call output to the candidate
    /// sideband. Success proves only local WebSocket write completion. It does
    /// not prove provider receipt, processing, or automatic continuation.
    pub async fn send_function_call_output(
        &self,
        call_id: Gate0CandidateOpaqueCallId,
        output: impl Into<String>,
    ) -> Result<Gate0CandidateFunctionOutputWrite, Gate0CandidateError> {
        let output = output.into();
        if output.len() > MAX_FUNCTION_OUTPUT_BYTES {
            return Err(Gate0CandidateError::InvalidFunctionOutput);
        }
        self.sender
            .send(&candidate_function_output_event(
                call_id.into_inner(),
                output,
            ))
            .await?;
        Ok(Gate0CandidateFunctionOutputWrite::LocalWriteCompleted)
    }

    pub async fn close(&self) -> Result<(), Gate0CandidateError> {
        self.sender.close().await.map_err(Gate0CandidateError::from)
    }
}

/// Opaque server-issued call field copied from one ephemeral raw Gate0 event.
/// Construction does not type or validate the unknown event schema.
#[derive(Clone, PartialEq, Eq)]
pub struct Gate0CandidateOpaqueCallId(FunctionCallId);

impl Gate0CandidateOpaqueCallId {
    pub fn try_from_observed_raw_field(
        value: impl Into<String>,
    ) -> Result<Self, Gate0CandidateError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Gate0CandidateError::InvalidFunctionOutput);
        }
        Ok(Self(FunctionCallId::new(value)))
    }

    fn into_inner(self) -> FunctionCallId {
        self.0
    }
}

impl std::fmt::Debug for Gate0CandidateOpaqueCallId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateOpaqueCallId(<redacted>)")
    }
}

fn candidate_function_output_event(call_id: FunctionCallId, output: String) -> ClientEvent {
    ClientEvent::DelegationFunctionCallOutput(DelegationFunctionCallOutput::new(
        FunctionCallOutput::new(call_id, output),
    ))
}

/// The strongest positive fact returned by candidate output custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate0CandidateFunctionOutputWrite {
    LocalWriteCompleted,
}

#[derive(Default)]
struct Gate0CandidateCorrelations {
    next_turn: u64,
    next_delegation: u64,
    next_transcript_item: u64,
    next_output_transcript_item: u64,
    turns: HashMap<String, Gate0CandidateTurnRef>,
}

impl Gate0CandidateCorrelations {
    fn turn(&mut self, provider_id: &str) -> Result<Gate0CandidateTurnRef, Gate0CandidateError> {
        if let Some(turn) = self.turns.get(provider_id) {
            return Ok(turn.clone());
        }
        if provider_id.trim().is_empty() {
            return Err(Gate0CandidateError::ProtocolDrift);
        }
        self.next_turn = self.next_turn.saturating_add(1);
        let turn = Gate0CandidateTurnRef {
            adapter_key: format!("turn:{}", self.next_turn),
            provider_id: provider_id.to_string(),
        };
        self.turns.insert(provider_id.to_string(), turn.clone());
        Ok(turn)
    }

    fn lower(
        &mut self,
        received: ReceivedServerEvent,
    ) -> Result<Gate0CandidateObservation, Gate0CandidateError> {
        let carrier = Gate0CandidateEventCarrier::from(received.carrier());
        let byte_count = received.byte_count();
        if byte_count == 0 || byte_count > MAX_RAW_JSON_EVENT_BYTES {
            return Err(Gate0CandidateError::ProtocolDrift);
        }
        let event = received.into_event();
        match event {
            ServerEvent::SessionStarted(_) => Ok(Gate0CandidateObservation::SessionStarted),
            ServerEvent::TurnCreated(created) => Ok(Gate0CandidateObservation::TurnStarted {
                turn: self.turn(&created.turn.id)?,
                role: created.turn.role,
            }),
            ServerEvent::TurnDelta(delta) => {
                if delta.delta.trim().is_empty() {
                    return Err(Gate0CandidateError::ProtocolDrift);
                }
                Ok(Gate0CandidateObservation::TurnDelta {
                    turn: self.turn(&delta.turn_id)?,
                    delta: delta.delta,
                })
            }
            ServerEvent::DelegationCreated(created) => {
                let turn = self.turn(&created.item.user_bidi_turn_id)?;
                if created.item.id.trim().is_empty() {
                    return Err(Gate0CandidateError::ProtocolDrift);
                }
                self.next_delegation = self.next_delegation.saturating_add(1);
                let delegation = Gate0CandidateDelegationRef {
                    adapter_key: format!("delegation:{}", self.next_delegation),
                    _provider_id: created.item.id,
                };
                Ok(Gate0CandidateObservation::DelegationCreated {
                    turn,
                    delegation,
                    target_client: created.item.target == "client",
                })
            }
            ServerEvent::TurnDone(done) => {
                if done.turn.transcript.trim().is_empty() {
                    return Err(Gate0CandidateError::ProtocolDrift);
                }
                Ok(Gate0CandidateObservation::TurnDone {
                    turn: self.turn(&done.turn.id)?,
                    role: done.turn.role,
                    transcript: done.turn.transcript,
                })
            }
            ServerEvent::DelegationContextAppended(_) => Err(Gate0CandidateError::ProtocolDrift),
            ServerEvent::InputTranscriptAdded(added) => {
                if added.item.id.trim().is_empty() || added.item.text.trim().is_empty() {
                    return Err(Gate0CandidateError::ProtocolDrift);
                }
                self.next_transcript_item = self.next_transcript_item.saturating_add(1);
                Ok(Gate0CandidateObservation::UserTranscriptAdded {
                    item: Gate0CandidateTranscriptItemRef {
                        adapter_key: format!("transcript:{}", self.next_transcript_item),
                        _provider_id: added.item.id,
                    },
                    text: added.item.text,
                })
            }
            ServerEvent::OutputTranscriptAdded(added) => {
                if added.item.id.trim().is_empty()
                    || added.item.text.trim().is_empty()
                    || added.end_ms < added.start_ms
                {
                    return Err(Gate0CandidateError::ProtocolDrift);
                }
                self.next_output_transcript_item =
                    self.next_output_transcript_item.saturating_add(1);
                Ok(Gate0CandidateObservation::OutputTranscriptAdded {
                    item: Gate0CandidateOutputTranscriptItemRef {
                        adapter_key: format!(
                            "output-transcript:{}",
                            self.next_output_transcript_item
                        ),
                        _provider_id: added.item.id,
                    },
                    start_ms: added.start_ms,
                    end_ms: added.end_ms,
                    text: added.item.text,
                })
            }
            ServerEvent::SessionContextAppended(_) => Ok(Gate0CandidateObservation::Other),
            ServerEvent::Unknown(unknown) => Ok(Gate0CandidateObservation::RawUnknownEvent(
                Gate0CandidateRawUnknownEvent {
                    carrier,
                    byte_count,
                    kind: unknown.kind().to_string(),
                    raw: unknown.raw().clone(),
                },
            )),
        }
    }
}

/// Redacted private-protocol turn identity. No provider identifier accessor
/// exists; only an adapter-local key can leave the provider boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct Gate0CandidateTurnRef {
    adapter_key: String,
    provider_id: String,
}

impl Gate0CandidateTurnRef {
    #[must_use]
    pub fn adapter_key(&self) -> &str {
        &self.adapter_key
    }
}

impl std::fmt::Debug for Gate0CandidateTurnRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateTurnRef(<redacted>)")
    }
}

/// Redacted private-protocol delegation identity.
#[derive(Clone, PartialEq, Eq)]
pub struct Gate0CandidateDelegationRef {
    adapter_key: String,
    _provider_id: String,
}

impl Gate0CandidateDelegationRef {
    #[must_use]
    pub fn adapter_key(&self) -> &str {
        &self.adapter_key
    }
}

impl std::fmt::Debug for Gate0CandidateDelegationRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateDelegationRef(<redacted>)")
    }
}

/// Redacted transcript item identity. The private event carries no turn id,
/// so this ref is deliberately not convertible to a turn ref.
#[derive(Clone, PartialEq, Eq)]
pub struct Gate0CandidateTranscriptItemRef {
    adapter_key: String,
    _provider_id: String,
}

impl Gate0CandidateTranscriptItemRef {
    #[must_use]
    pub fn adapter_key(&self) -> &str {
        &self.adapter_key
    }
}

impl std::fmt::Debug for Gate0CandidateTranscriptItemRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateTranscriptItemRef(<redacted>)")
    }
}

/// Redacted identity for one timed assistant output-transcript segment. The
/// private event has no provider parent field, so association with an active
/// assistant response is proved only by serialized role-bearing event order.
#[derive(Clone, PartialEq, Eq)]
pub struct Gate0CandidateOutputTranscriptItemRef {
    adapter_key: String,
    _provider_id: String,
}

impl Gate0CandidateOutputTranscriptItemRef {
    #[must_use]
    pub fn adapter_key(&self) -> &str {
        &self.adapter_key
    }
}

impl std::fmt::Debug for Gate0CandidateOutputTranscriptItemRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateOutputTranscriptItemRef(<redacted>)")
    }
}

/// Mechanical carrier observed by the unqualified direct-protocol harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate0CandidateEventCarrier {
    Sideband,
    OrderedOaiEvents,
}

impl From<EventCarrier> for Gate0CandidateEventCarrier {
    fn from(carrier: EventCarrier) -> Self {
        match carrier {
            EventCarrier::Sideband => Self::Sideband,
            EventCarrier::OrderedOaiEvents => Self::OrderedOaiEvents,
        }
    }
}

/// Bounded raw event retained only in the Gate0 process until its private
/// discriminant and fields have been directly qualified. No typed function
/// call or semantic correlation is inferred from this payload.
#[derive(Clone, PartialEq)]
pub struct Gate0CandidateRawUnknownEvent {
    carrier: Gate0CandidateEventCarrier,
    byte_count: usize,
    kind: String,
    raw: serde_json::Value,
}

impl Gate0CandidateRawUnknownEvent {
    #[must_use]
    pub const fn carrier(&self) -> Gate0CandidateEventCarrier {
        self.carrier
    }

    #[must_use]
    pub const fn byte_count(&self) -> usize {
        self.byte_count
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Borrow the bounded raw JSON for ephemeral local Gate0 collection. The
    /// value must never enter logs, durable evidence, or shipping projection.
    #[must_use]
    pub const fn raw_json(&self) -> &serde_json::Value {
        &self.raw
    }
}

impl std::fmt::Debug for Gate0CandidateRawUnknownEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate0CandidateRawUnknownEvent")
            .field("carrier", &self.carrier)
            .field("byte_count", &self.byte_count)
            .field("kind", &"<preserved-redacted>")
            .field("raw", &"<redacted>")
            .field("qualification", &"live-unqualified")
            .finish()
    }
}

/// Candidate-only lowering of the private GPT Live event sequence. Payloads
/// needed for the real worker remain in-process and never enter evidence.
#[derive(Clone)]
pub enum Gate0CandidateObservation {
    SessionStarted,
    TurnStarted {
        turn: Gate0CandidateTurnRef,
        role: String,
    },
    TurnDelta {
        turn: Gate0CandidateTurnRef,
        delta: String,
    },
    DelegationCreated {
        turn: Gate0CandidateTurnRef,
        delegation: Gate0CandidateDelegationRef,
        target_client: bool,
    },
    TurnDone {
        turn: Gate0CandidateTurnRef,
        role: String,
        transcript: String,
    },
    UserTranscriptAdded {
        item: Gate0CandidateTranscriptItemRef,
        text: String,
    },
    OutputTranscriptAdded {
        item: Gate0CandidateOutputTranscriptItemRef,
        start_ms: u64,
        end_ms: u64,
        text: String,
    },
    RawUnknownEvent(Gate0CandidateRawUnknownEvent),
    Other,
}

impl std::fmt::Debug for Gate0CandidateObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::SessionStarted => "session_started",
            Self::TurnStarted { .. } => "turn_started",
            Self::TurnDelta { .. } => "turn_delta",
            Self::DelegationCreated { .. } => "delegation_created",
            Self::TurnDone { .. } => "turn_done",
            Self::UserTranscriptAdded { .. } => "user_transcript_added",
            Self::OutputTranscriptAdded { .. } => "output_transcript_added",
            Self::RawUnknownEvent(_) => "raw_unknown_event",
            Self::Other => "other",
        };
        formatter
            .debug_struct("Gate0CandidateObservation")
            .field("kind", &kind)
            .field("payload", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Gate0CandidateError {
    #[error("Gate0 candidate binding-use witness does not match the resolved target")]
    BindingUseMismatch,
    #[error("Gate0 candidate target is not an experimental realtime ChatGPT target")]
    TargetRejected,
    #[error("Gate0 candidate credential is unavailable")]
    MissingCredential,
    #[error("Gate0 candidate transport configuration failed")]
    TransportConfiguration,
    #[error("Gate0 candidate open input is invalid")]
    InvalidOpen,
    #[error("Gate0 candidate Responses configuration is invalid")]
    InvalidResponsesConfiguration,
    #[error("Gate0 candidate private transport failed")]
    Transport,
    #[error("Gate0 candidate sideband closed before session.started")]
    SessionStartedMissing,
    #[error("Gate0 candidate function-call output is invalid")]
    InvalidFunctionOutput,
    #[error("Gate0 candidate private protocol drifted")]
    ProtocolDrift,
}

impl From<TransportError> for Gate0CandidateError {
    fn from(_: TransportError) -> Self {
        Self::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_llm_core::provider_runtime::AdmittedExperimentalRealtimeTarget;
    use oai_rt_rs::experimental::gpt_live::{
        CodecError, MAX_BRIDGE_ARGUMENT_BYTES, decode_bridge_arguments,
        decode_received_server_event, encode_client_event,
    };

    trait AmbiguousIfCandidateConverts<Marker> {
        fn witness() {}
    }

    impl<T: ?Sized> AmbiguousIfCandidateConverts<()> for T {}
    impl<T: ?Sized + Into<AdmittedExperimentalRealtimeTarget>> AmbiguousIfCandidateConverts<u8> for T {}

    #[test]
    fn candidate_target_has_no_conversion_into_shipping_admission() {
        let _ = <Gate0CandidateRealtimeTarget as AmbiguousIfCandidateConverts<_>>::witness;
    }

    fn candidate_config() -> Gate0ResponsesCandidateConfig {
        Gate0ResponsesCandidateConfig::try_new("bridge-model-private")
            .expect("valid candidate Responses configuration")
    }

    #[test]
    fn responses_candidate_config_is_exact_and_explicitly_unqualified() {
        let config = candidate_config();

        assert_eq!(
            config.qualification(),
            Gate0CandidateQualification::LiveUnqualified
        );
        assert_eq!(config.delegation_type(), "responses");
        assert!(config.bridge_model_present());
        assert_eq!(config.tool_count(), 1);
        assert_eq!(config.tool_name(), Some(GATE0_RESPONSES_BRIDGE_TOOL));
        assert!(config.strict_arguments_schema());
        assert!(config.exact_authority_free_shape());

        let wire = serde_json::to_value(config.delegation()).expect("serialize delegation");
        assert_eq!(wire["type"], "responses");
        assert_eq!(wire["responses"]["model"], "bridge-model-private");
        assert!(wire["responses"].get("instructions").is_none());
        assert_eq!(wire["responses"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(
            wire["responses"]["tools"][0]["name"],
            GATE0_RESPONSES_BRIDGE_TOOL
        );
        assert_eq!(
            wire["responses"]["tools"][0]["parameters"],
            strict_bridge_parameters()
        );
    }

    #[test]
    fn responses_candidate_configuration_and_raw_observation_debug_are_redacted() {
        let config = candidate_config();
        let config_debug = format!("{config:?}");
        assert!(!config_debug.contains("bridge-model-private"));
        assert!(config_debug.contains("live-unqualified"));

        let raw = r#"{"type":"private.responses.call","call_id":"provider-secret","arguments":"conversation-secret"}"#;
        let received = decode_received_server_event(EventCarrier::Sideband, raw)
            .expect("bounded unknown event");
        let observation = Gate0CandidateCorrelations::default()
            .lower(received)
            .expect("candidate lowering");
        let Gate0CandidateObservation::RawUnknownEvent(observation) = observation else {
            panic!("unknown event must remain raw")
        };
        let debug = format!("{observation:?}");
        assert!(!debug.contains("private.responses.call"));
        assert!(!debug.contains("provider-secret"));
        assert!(!debug.contains("conversation-secret"));
        assert_eq!(observation.carrier(), Gate0CandidateEventCarrier::Sideband);
        assert_eq!(observation.byte_count(), raw.len());
        assert_eq!(observation.kind(), "private.responses.call");
        assert_eq!(observation.raw_json()["call_id"], "provider-secret");
    }

    #[test]
    fn raw_unknown_event_preserves_ordered_carrier_without_promoting_schema() {
        let raw = r#"{"type":"candidate.only.unknown","opaque":{"x":1}}"#;
        let received = decode_received_server_event(EventCarrier::OrderedOaiEvents, raw)
            .expect("bounded unknown event");
        let observation = Gate0CandidateCorrelations::default()
            .lower(received)
            .expect("candidate lowering");
        let Gate0CandidateObservation::RawUnknownEvent(observation) = observation else {
            panic!("unknown event must remain raw")
        };
        assert_eq!(
            observation.carrier(),
            Gate0CandidateEventCarrier::OrderedOaiEvents
        );
        assert_eq!(observation.kind(), "candidate.only.unknown");
        assert_eq!(observation.raw_json()["opaque"]["x"], 1);
    }

    #[test]
    fn exact_function_call_output_envelope_and_bounds_are_mechanical() {
        let opaque =
            Gate0CandidateOpaqueCallId::try_from_observed_raw_field("provider-call-secret")
                .expect("nonempty raw call field");
        assert!(!format!("{opaque:?}").contains("provider-call-secret"));
        let event = candidate_function_output_event(
            opaque.into_inner(),
            "meerkat-result-secret".to_string(),
        );
        let encoded = encode_client_event(&event).expect("bounded output envelope");
        let value: serde_json::Value = serde_json::from_str(&encoded).expect("valid JSON");
        assert_eq!(value["type"], "delegation.function_call_output.create");
        assert_eq!(value["item"]["type"], "function_call_output");
        assert_eq!(value["item"]["call_id"], "provider-call-secret");
        assert_eq!(value["item"]["output"], "meerkat-result-secret");
        assert!(value.get("response").is_none());

        let at_bound = candidate_function_output_event(
            FunctionCallId::new("call"),
            "x".repeat(MAX_FUNCTION_OUTPUT_BYTES),
        );
        assert!(encode_client_event(&at_bound).is_ok());
        let oversized = candidate_function_output_event(
            FunctionCallId::new("call"),
            "x".repeat(MAX_FUNCTION_OUTPUT_BYTES + 1),
        );
        assert!(matches!(
            encode_client_event(&oversized),
            Err(CodecError::OversizedFunctionOutput)
        ));
    }

    #[test]
    fn raw_event_and_decoded_bridge_argument_bounds_fail_closed() {
        let oversized_raw = format!(
            r#"{{"type":"unknown","padding":"{}"}}"#,
            "x".repeat(MAX_RAW_JSON_EVENT_BYTES)
        );
        assert!(matches!(
            decode_received_server_event(EventCarrier::Sideband, &oversized_raw),
            Err(CodecError::OversizedRawEvent)
        ));

        let at_bound = format!(
            r#"{{"request":"{}"}}"#,
            "x".repeat(MAX_BRIDGE_ARGUMENT_BYTES)
        );
        let decoded = decode_bridge_arguments(&at_bound).expect("decoded request at bound");
        assert_eq!(decoded.request().len(), MAX_BRIDGE_ARGUMENT_BYTES);
        let oversized = format!(
            r#"{{"request":"{}"}}"#,
            "x".repeat(MAX_BRIDGE_ARGUMENT_BYTES + 1)
        );
        assert!(matches!(
            decode_bridge_arguments(&oversized),
            Err(CodecError::OversizedBridgeArguments)
        ));
        assert!(matches!(
            decode_bridge_arguments(r#"{"request":"ok","member_id":"shadow"}"#),
            Err(CodecError::MalformedBridgeArguments)
        ));
    }
}
