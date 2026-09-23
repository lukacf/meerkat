//! Provider-neutral GPT Live broker vocabulary shared by the public Live
//! broker and the deprecated experimental (private-protocol) broker.
//!
//! Observations are sanitized: no provider call, session, turn,
//! transcript-item, or delegation identifier leaks through `Debug`, and text
//! is semantic observation content rather than a raw payload. Errors carry a
//! sanitized terminal class only.

/// Sanitized terminal classification for broker transport mechanics.
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
pub struct GptLiveAppendToken(pub(crate) u64);

impl std::fmt::Debug for GptLiveAppendToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GptLiveAppendToken(<local>)")
    }
}

/// Opaque delegation reference that can only be minted by this adapter.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GptLiveDelegationRef(pub(crate) String);

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
pub struct GptLiveTranscriptItemRef(pub(crate) String);

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
pub struct GptLiveTurnRef(pub(crate) String);

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
    #[cfg(feature = "experimental-gpt-live")]
    pub(crate) fn from_provider_role(role: &str) -> Self {
        match role {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => Self::Unknown,
        }
    }
}

/// Sanitized provider observations emitted by a GPT Live sideband broker.
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
    /// Closing interrupted the append. This does not prove zero consumption.
    SessionContextAppendRejected {
        token: GptLiveAppendToken,
    },
    /// Every fragment of a quiet factual-context append was acknowledged.
    ThinkingContextAppendAcknowledged {
        token: GptLiveAppendToken,
    },
    /// A native error rejected at least one fragment; other fragments may
    /// already be consumed. This is ambiguous aggregate delivery, not a
    /// definitive pre-delivery refusal, even after a local close request.
    /// It does not authorize replay.
    ThinkingContextAppendRejected {
        token: GptLiveAppendToken,
    },
    /// Confirmed provider session closure interrupted an unresolved append.
    /// Prior fragments may be consumed; this does not authorize replay.
    ThinkingContextAppendInterruptedByClose {
        token: GptLiveAppendToken,
    },
    /// Every fragment of a trusted instructions-lane append (the historical
    /// bootstrap summary) was acknowledged.
    InstructionsContextAppendAcknowledged {
        token: GptLiveAppendToken,
    },
    /// A native error rejected at least one instructions fragment; other
    /// fragments may already be consumed. Ambiguous aggregate delivery.
    InstructionsContextAppendRejected {
        token: GptLiveAppendToken,
    },
    /// Confirmed provider session closure interrupted an unresolved
    /// instructions append. Prior fragments may be consumed.
    InstructionsContextAppendInterruptedByClose {
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
    /// work or establish canonical transcript commitment. Public sessions
    /// join the delegation to a locally synthesized user turn.
    ///
    /// `transcript` is the terminated user turn (the canonical row).
    /// `request_transcript` is the executor input: every user transcript
    /// delta received since the previous `session.delegation.created` on
    /// this session (or since open), regardless of assistant output in
    /// between, because the provider's backchannels are designed behaviour
    /// and never end the user's request. `assistant_context` is the
    /// assistant transcript received in that same window, so the executor
    /// can see what was already answered natively; it is never merged into
    /// the request.
    ClientDelegationFinal {
        delegation: GptLiveDelegationRef,
        target: GptLiveDelegationTarget,
        turn: GptLiveTurnRef,
        transcript: String,
        request_transcript: String,
        assistant_context: String,
    },
    DelegationActionableInputUnsupported {
        delegation: GptLiveDelegationRef,
    },
    DelegationContextAppendAcknowledged {
        token: GptLiveAppendToken,
    },
    /// Closing interrupted the append. This does not authorize replay.
    DelegationContextAppendRejected {
        token: GptLiveAppendToken,
    },
    UnsupportedProviderEvent,
}

impl std::fmt::Debug for GptLiveBrokerObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::SessionReady => "session_ready",
            Self::SessionContextAppendAcknowledged { .. } => "session_context_append_acknowledged",
            Self::SessionContextAppendRejected { .. } => "session_context_append_rejected",
            Self::ThinkingContextAppendAcknowledged { .. } => {
                "thinking_context_append_acknowledged"
            }
            Self::ThinkingContextAppendRejected { .. } => "thinking_context_append_rejected",
            Self::ThinkingContextAppendInterruptedByClose { .. } => {
                "thinking_context_append_interrupted_by_close"
            }
            Self::InstructionsContextAppendAcknowledged { .. } => {
                "instructions_context_append_acknowledged"
            }
            Self::InstructionsContextAppendRejected { .. } => {
                "instructions_context_append_rejected"
            }
            Self::InstructionsContextAppendInterruptedByClose { .. } => {
                "instructions_context_append_interrupted_by_close"
            }
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
            Self::DelegationContextAppendRejected { .. } => "delegation_context_append_rejected",
            Self::UnsupportedProviderEvent => "unsupported_provider_event",
        };
        formatter
            .debug_struct("GptLiveBrokerObservation")
            .field("kind", &kind)
            .field("payload", &"<redacted>")
            .finish()
    }
}

/// Sanitized failure surface for browser bootstrap and sideband mechanics.
#[derive(thiserror::Error)]
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
    #[error("GPT Live transport terminated")]
    Transport { class: GptLiveBrokerTerminalClass },
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

pub(crate) fn protocol_error() -> GptLiveBrokerError {
    GptLiveBrokerError::Transport {
        class: GptLiveBrokerTerminalClass::Protocol,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnknownProviderEventSummary {
    pub(crate) event_kind_sha256: String,
    pub(crate) error_class: &'static str,
    pub(crate) top_level_field_count: usize,
    pub(crate) normalized_json_bytes: usize,
    pub(crate) message_bytes: usize,
}

/// Redacting fingerprint of an unsupported provider event: only a hash of
/// the discriminator, an error class, and byte counts survive.
pub(crate) fn summarize_unknown_provider_event(
    kind: &str,
    raw: &serde_json::Value,
) -> UnknownProviderEventSummary {
    use sha2::{Digest, Sha256};

    let message = raw
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
    } else if kind == "error" {
        "other_provider_error"
    } else {
        "unsupported_event"
    };
    UnknownProviderEventSummary {
        event_kind_sha256: format!("{:x}", Sha256::digest(kind.as_bytes())),
        error_class,
        top_level_field_count: raw.as_object().map_or(0, serde_json::Map::len),
        normalized_json_bytes: serde_json::to_vec(raw).map_or(0, |bytes| bytes.len()),
        message_bytes: message.len(),
    }
}

pub(crate) fn require_context(text: impl Into<String>) -> Result<String, GptLiveBrokerError> {
    let text = text.into();
    if text.trim().is_empty() {
        return Err(GptLiveBrokerError::MissingContext);
    }
    Ok(text)
}
