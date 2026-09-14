//! Bounded completion records for the independent Live ledger.
//!
//! A fit is encoding evidence only. Generated authority must reserve capacity
//! before creating an obligation and commits these bytes with the head CAS.

use std::fmt;

use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::evidence::LiveContentDigest;
use meerkat_core::live_execution::observation::LiveUsageSnapshot;
use meerkat_core::live_execution::request::LiveRequestCancellationReason;
use meerkat_core::live_observation::LiveObservationSeq;
use meerkat_core::{
    SessionId,
    lifecycle::{InputId, RunId},
    ops::OperationId,
};
use serde::{Deserialize, Deserializer, Serialize};

use super::transcript::{LiveDiscontinuity, LiveLedgerFormatV1};
use crate::live_delivery::{
    LiveContextChunkDeliveryState, LiveContinuationState, LiveResultDeliveryState,
};
use crate::live_resources::{LiveCompletionObligation, LiveResourceCharge};

macro_rules! completion_states {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

pub const LIVE_RESULT_MAX_BYTES: usize = 16 * 1024;
pub const LIVE_CONTEXT_CHUNK_MAX_BYTES: usize = 400;
pub const LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES: usize = 1024;
pub const LIVE_COMPLETION_ID_MAX_BYTES: usize = 128;

/// Exact UTF-8 content with a decoded byte ceiling, not a JSON byte estimate.
/// Empty result/diagnostic text is valid. Display deliberately stays redacted.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LiveCompletionText<const MAX_BYTES: usize>(Box<str>);

impl<const MAX_BYTES: usize> LiveCompletionText<MAX_BYTES> {
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, LiveCompletionEncodingError> {
        let value = value.into();
        if value.len() > MAX_BYTES {
            return Err(LiveCompletionEncodingError::TextTooLarge {
                max_bytes: MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const MAX_BYTES: usize> fmt::Debug for LiveCompletionText<MAX_BYTES> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveCompletionText")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl<'de, const MAX_BYTES: usize> Deserialize<'de> for LiveCompletionText<MAX_BYTES> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?.into_boxed_str())
            .map_err(serde::de::Error::custom)
    }
}

impl<const MAX_BYTES: usize> schemars::JsonSchema for LiveCompletionText<MAX_BYTES> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("LiveCompletionText_{MAX_BYTES}").into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = generator.subschema_for::<String>();
        schema.insert("maxLength".into(), serde_json::json!(MAX_BYTES));
        schema.insert("x-max-utf8-bytes".into(), serde_json::json!(MAX_BYTES));
        schema
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRequestCompletionFact {
    /// References the ordinary owner's complete finalized outcome, including
    /// structured output, accounting, stop causes, and extraction failures.
    /// This is neither a copied result nor a certified public artifact.
    OrdinaryTerminal {
        input_id: InputId,
        run_id: RunId,
        receipt_digest: LiveCompletionText<64>,
    },
    Refused {
        reason: LiveRequestRefusal,
    },
    CancelledWithoutRun {
        reason: LiveRequestCancellationReason,
    },
    Completed {
        input_id: InputId,
        run_id: RunId,
        result: LiveCompletionText<LIVE_RESULT_MAX_BYTES>,
    },
    Failed {
        input_id: InputId,
        run_id: RunId,
        detail: LiveCompletionText<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>,
    },
    Cancelled {
        input_id: InputId,
        run_id: RunId,
        reason: LiveRequestCancellationReason,
    },
    Held {
        input_id: InputId,
        reason: LiveRequestHold,
    },
    AdmissionUnconfirmed {},
    /// References the finalized ordinary owner for an admitted input that never ran.
    OrdinaryRunlessTerminal {
        input_id: InputId,
        receipt_digest: LiveCompletionText<64>,
    },
}

completion_states! { LiveRequestRefusal {
    EmptySnapshot,
    DiscontinuousSnapshot,
    RequestBudgetExceeded,
    PermissionDenied,
    SourcePayloadConflict,
    IngressClosed,
    ResourceCapacityRefused,
    InvalidFunctionRequest,
}}

completion_states! { LiveRequestHold {
    AdmissionUnconfirmed,
    ExecutionUnconfirmed,
    EffectOutcomeUnknown,
    ScopeRestorationFailed,
    OrdinaryCompletionUnclassified,
    OrdinaryFinalizationFailed,
}}

completion_states! { LivePhysicalEffectOutcome {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
    NotStarted,
}}

completion_states! { LiveChannelControlOutcome {
    IngressClosed,
    GrantRevoked,
    RecoveryFenced,
    DeliveryFenced,
    StorageQuotaReached,
    Activated,
}}

/// Terminal/control payloads reference previously persisted source, scope,
/// callback and attempt records by local IDs. Provider keys and argument
/// bodies are not redundantly copied into every completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveCompletionEvent {
    ChannelControl {
        outcome: LiveChannelControlOutcome,
        diagnostic: LiveCompletionText<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>,
    },
    ChannelDiscontinuity {
        discontinuity: LiveDiscontinuity,
    },
    ChannelUsage {
        snapshot: LiveUsageSnapshot,
    },
    ChannelProviderStarted {
        #[schemars(with = "String", extend("minLength" = 1, "maxLength" = LIVE_COMPLETION_ID_MAX_BYTES, "x-max-utf8-bytes" = LIVE_COMPLETION_ID_MAX_BYTES))]
        provider_session: LiveCompletionText<LIVE_COMPLETION_ID_MAX_BYTES>,
    },
    ChannelProviderDiagnostic {
        diagnostic: meerkat_core::live_execution::backend::LiveProviderDiagnostic,
    },
    RequestOutcome {
        request_id: OperationId,
        outcome: LiveRequestCompletionFact,
    },
    EffectTerminal {
        claim_id: OperationId,
        request_id: OperationId,
        outcome: LivePhysicalEffectOutcome,
        token_accounting: meerkat_core::execution_scope::ScopedEffectTokenAccounting,
        diagnostic: LiveCompletionText<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>,
    },
    CallbackSuspended {
        claim_id: OperationId,
        request_id: OperationId,
        input_id: InputId,
        run_id: RunId,
        batch_digest: LiveContentDigest,
    },
    FunctionOutput {
        attempt_id: OperationId,
        request_id: OperationId,
        output: LiveCompletionText<LIVE_RESULT_MAX_BYTES>,
    },
    FunctionOutputSettlement {
        attempt_id: OperationId,
        state: LiveResultDeliveryState,
    },
    ContinuationSettlement {
        attempt_id: OperationId,
        state: LiveContinuationState,
    },
    ContextChunk {
        attempt_id: OperationId,
        content: LiveCompletionText<LIVE_CONTEXT_CHUNK_MAX_BYTES>,
    },
    ContextSettlement {
        attempt_id: OperationId,
        state: LiveContextChunkDeliveryState,
    },
    CallbackContinuationSettlement {
        request_id: OperationId,
        suspended_run_id: RunId,
        outcome: LiveCallbackContinuationOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveCallbackContinuationOutcome {
    Admitted { input_id: InputId },
    CancelledWithoutInput {},
    ResourceCapacityRefused {},
    ScopeRestorationFailed {},
    AdmissionUnconfirmed {},
}

impl LiveCompletionEvent {
    pub const fn obligation(&self) -> LiveCompletionObligation {
        match self {
            Self::ChannelControl { .. }
            | Self::ChannelDiscontinuity { .. }
            | Self::ChannelUsage { .. }
            | Self::ChannelProviderStarted { .. }
            | Self::ChannelProviderDiagnostic { .. } => LiveCompletionObligation::ChannelControl,
            Self::RequestOutcome { .. } => LiveCompletionObligation::RequestChain,
            Self::EffectTerminal { .. } | Self::CallbackSuspended { .. } => {
                LiveCompletionObligation::EffectStart
            }
            Self::FunctionOutput { .. } | Self::FunctionOutputSettlement { .. } => {
                LiveCompletionObligation::FunctionOutput
            }
            Self::ContinuationSettlement { .. } => LiveCompletionObligation::Continuation,
            Self::ContextChunk { .. } | Self::ContextSettlement { .. } => {
                LiveCompletionObligation::ContextChunk
            }
            Self::CallbackContinuationSettlement { .. } => {
                LiveCompletionObligation::CallbackContinuation
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiveCompletionRecord {
    pub format: LiveLedgerFormatV1,
    pub session_id: SessionId,
    #[schemars(extend("minLength" = 1, "maxLength" = LIVE_COMPLETION_ID_MAX_BYTES, "x-max-utf8-bytes" = LIVE_COMPLETION_ID_MAX_BYTES))]
    pub channel_id: LiveChannelId,
    pub sequence: LiveObservationSeq,
    pub event: LiveCompletionEvent,
}

impl LiveCompletionRecord {
    /// Same bounded JSON encoder as observation fit, history replies and
    /// cursors. No success-shaped truncation or second accounting serializer.
    pub fn encode(&self) -> Result<EncodedLiveCompletion, LiveCompletionEncodingError> {
        if self.channel_id.as_str().is_empty()
            || self.channel_id.as_str().len() > LIVE_COMPLETION_ID_MAX_BYTES
        {
            return Err(LiveCompletionEncodingError::InvalidChannel);
        }
        if let LiveCompletionEvent::ChannelProviderStarted { provider_session } = &self.event
            && provider_session.as_str().is_empty()
        {
            return Err(LiveCompletionEncodingError::InvalidProviderSession);
        }
        if let LiveCompletionEvent::ChannelDiscontinuity { discontinuity } = &self.event {
            let matches = match discontinuity {
                LiveDiscontinuity::KnownLocalGap { channel_id, .. } => {
                    channel_id == &self.channel_id
                }
                LiveDiscontinuity::UnknownExtentCrashDiscontinuity {
                    last_accepted_head,
                    old_incarnation,
                } => {
                    old_incarnation == &self.channel_id
                        && last_accepted_head.session_id == self.session_id
                }
            };
            if !matches {
                return Err(LiveCompletionEncodingError::DiscontinuityIdentityMismatch);
            }
        }
        let bytes = meerkat_contracts::wire::live_observation::LiveObservationWireCodecV1::encode_ledger_record(self)?;
        let charge = LiveResourceCharge::for_event_record(&bytes)?;
        Ok(EncodedLiveCompletion {
            obligation: self.event.obligation(),
            bytes,
            charge,
        })
    }
}

pub struct EncodedLiveCompletion {
    obligation: LiveCompletionObligation,
    bytes: Vec<u8>,
    charge: LiveResourceCharge,
}

impl fmt::Debug for EncodedLiveCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedLiveCompletion")
            .field("obligation", &self.obligation)
            .field("charge", &self.charge)
            .finish_non_exhaustive()
    }
}

impl EncodedLiveCompletion {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn obligation(&self) -> LiveCompletionObligation {
        self.obligation
    }

    pub fn charge(&self) -> LiveResourceCharge {
        self.charge
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveCompletionEncodingError {
    #[error("live completion text exceeds its decoded byte bound {max_bytes}")]
    TextTooLarge { max_bytes: usize },
    #[error("live completion channel id is empty or exceeds its byte bound")]
    InvalidChannel,
    #[error("live provider session id is empty")]
    InvalidProviderSession,
    #[error("live discontinuity does not match its record session and channel")]
    DiscontinuityIdentityMismatch,
    #[error(transparent)]
    Encoding(#[from] meerkat_contracts::wire::live_observation::LiveObservationEncodingError),
    #[error(transparent)]
    Arithmetic(#[from] crate::live_resources::LiveResourceArithmeticError),
}
