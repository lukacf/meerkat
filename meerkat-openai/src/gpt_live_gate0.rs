//! Non-shipping direct-protocol Gate0 candidate broker.
//!
//! This module is structurally separate from the qualified experimental
//! factory. Its target is not convertible to `AdmittedExperimentalRealtimeTarget`
//! and its factory never accepts that shipping carrier.

use std::collections::HashMap;

use meerkat_core::{AuthBindingUseWitness, ProviderAuthMetadata};
use meerkat_llm_core::provider_runtime::{NormalizedBackendKind, ResolvedRealtimeTarget};
use oai_rt_rs::experimental::gpt_live::{
    CallSession, ClientDelegation, ClientEvent, ContextChannel, CreateCallRequest,
    DelegationContextAppend, ExtraFields, GptLiveCredentials, GptLiveTransport, InputTextContent,
    ServerEvent, SessionAudio, SessionAudioOutput, SidebandHeaders, SidebandReceiver,
    SidebandSender, TransportError,
};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::OpenAiBackendKind;

/// Fixed declaration emitted by sanitized evidence. It is not a protocol
/// digest and cannot satisfy any qualified-build admission predicate.
pub const GATE0_CANDIDATE_CONTRACT: &str = "unqualified-direct-gate0-v1";

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
    transport: GptLiveTransport,
    credentials: GptLiveCredentials,
}

impl std::fmt::Debug for Gate0CandidateBrokerFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gate0CandidateBrokerFactory")
            .field("model", &"<registry-validated>")
            .field("transport", &"<private-transport>")
            .field("credentials", &"<redacted>")
            .field("qualification", &"unqualified-candidate")
            .finish()
    }
}

impl Gate0CandidateBrokerFactory {
    pub fn try_from_candidate(
        candidate: Gate0CandidateRealtimeTarget,
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
            transport,
            credentials,
        })
    }

    pub async fn answer(
        &self,
        offer_sdp: String,
        voice: String,
        instructions: Option<String>,
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
                delegation: Some(ClientDelegation {
                    delegation_type: "client".to_string(),
                    extra: ExtraFields::new(),
                }),
                instructions: instructions.filter(|value| !value.trim().is_empty()),
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
        let event = self.receiver.lock().await.next_event().await?;
        let Some(event) = event else {
            return Ok(None);
        };
        let mut correlations = self.correlations.lock().await;
        Ok(Some(correlations.lower(event)?))
    }

    /// Append one reconciled worker result to the exact observed delegation.
    /// The returned ref is opaque and later compared with the provider ack.
    pub async fn append_delegation_context(
        &self,
        delegation: &Gate0CandidateDelegationRef,
        text: impl Into<String>,
    ) -> Result<Gate0CandidateAppendRef, Gate0CandidateError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(Gate0CandidateError::InvalidAppend);
        }
        self.sender
            .send(&ClientEvent::DelegationContextAppend(
                DelegationContextAppend {
                    delegation_item_id: delegation.provider_id.clone(),
                    channel: Some(ContextChannel::Commentary),
                    content: vec![InputTextContent {
                        content_type: "input_text".to_string(),
                        text,
                        extra: ExtraFields::new(),
                    }],
                    extra: ExtraFields::new(),
                },
            ))
            .await?;
        Ok(Gate0CandidateAppendRef {
            delegation: delegation.clone(),
        })
    }

    pub async fn close(&self) -> Result<(), Gate0CandidateError> {
        self.sender.close().await.map_err(Gate0CandidateError::from)
    }
}

#[derive(Default)]
struct Gate0CandidateCorrelations {
    next_turn: u64,
    next_delegation: u64,
    next_transcript_item: u64,
    next_output_transcript_item: u64,
    turns: HashMap<String, Gate0CandidateTurnRef>,
    delegations: HashMap<String, Gate0CandidateDelegationRef>,
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
        event: ServerEvent,
    ) -> Result<Gate0CandidateObservation, Gate0CandidateError> {
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
                    provider_id: created.item.id.clone(),
                };
                self.delegations.insert(created.item.id, delegation.clone());
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
            ServerEvent::DelegationContextAppended(appended) => {
                let delegation = self
                    .delegations
                    .get(&appended.delegation_item_id)
                    .cloned()
                    .ok_or(Gate0CandidateError::ProtocolDrift)?;
                Ok(Gate0CandidateObservation::DelegationContextAppended { delegation })
            }
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
            ServerEvent::SessionContextAppended(_) | ServerEvent::Unknown(_) => {
                Ok(Gate0CandidateObservation::Other)
            }
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
    provider_id: String,
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

/// Opaque identity of the exact append attempted by the candidate harness.
#[derive(Clone, PartialEq, Eq)]
pub struct Gate0CandidateAppendRef {
    delegation: Gate0CandidateDelegationRef,
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

impl Gate0CandidateAppendRef {
    #[must_use]
    pub fn acknowledged_by(&self, delegation: &Gate0CandidateDelegationRef) -> bool {
        &self.delegation == delegation
    }
}

impl std::fmt::Debug for Gate0CandidateAppendRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gate0CandidateAppendRef(<redacted>)")
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
    DelegationContextAppended {
        delegation: Gate0CandidateDelegationRef,
    },
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
            Self::DelegationContextAppended { .. } => "delegation_context_appended",
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
    #[error("Gate0 candidate private transport failed")]
    Transport,
    #[error("Gate0 candidate sideband closed before session.started")]
    SessionStartedMissing,
    #[error("Gate0 candidate append input is invalid")]
    InvalidAppend,
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

    trait AmbiguousIfCandidateConverts<Marker> {
        fn witness() {}
    }

    impl<T: ?Sized> AmbiguousIfCandidateConverts<()> for T {}
    impl<T: ?Sized + Into<AdmittedExperimentalRealtimeTarget>> AmbiguousIfCandidateConverts<u8> for T {}

    #[test]
    fn candidate_target_has_no_conversion_into_shipping_admission() {
        let _ = <Gate0CandidateRealtimeTarget as AmbiguousIfCandidateConverts<_>>::witness;
    }
}
