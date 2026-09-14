//! Independent readiness and cumulative voice accounting facts.
//!
//! These records report evidence, not generated channel state. Provider
//! start, SDP answer delivery, and media observation are not interchangeable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveReadinessEvidence {
    pub provider: LiveProviderStartEvidence,
    pub answer: LiveAnswerDeliveryEvidence,
    pub media: LivePeerMediaEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveProviderStartEvidence {
    Awaiting,
    Started,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveAnswerDeliveryEvidence {
    NotRequired,
    Awaiting,
    Delivered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivePeerMediaEvidence {
    Unobserved,
    InboundObserved,
    OutboundObserved,
    BidirectionalObserved,
}

/// Provider cumulative seconds. Deliberately has no addition operation:
/// snapshots replace the prior account observation, not increment it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64")]
pub struct LiveVoiceDurationSeconds(f64);

#[cfg(feature = "schema")]
impl schemars::JsonSchema for LiveVoiceDurationSeconds {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LiveVoiceDurationSeconds".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let mut schema = generator.subschema_for::<f64>();
        schema.insert("minimum".into(), serde_json::json!(0.0));
        schema.insert("maximum".into(), serde_json::json!(f64::MAX));
        schema
    }
}

impl Eq for LiveVoiceDurationSeconds {}

impl LiveVoiceDurationSeconds {
    pub fn new(value: f64) -> Result<Self, LiveVoiceDurationError> {
        if !value.is_finite() || value < 0.0 {
            return Err(LiveVoiceDurationError);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for LiveVoiceDurationSeconds {
    type Error = LiveVoiceDurationError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// A close event is independent of the provider session snapshot's status.
/// An unconfirmed close cannot be serialized as a final zero-duration usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveUsageSnapshot {
    Periodic {
        cumulative_seconds: LiveVoiceDurationSeconds,
    },
    SessionClosed {
        cumulative_seconds: LiveVoiceDurationSeconds,
    },
    CloseUnconfirmed {
        last_observed_seconds: Option<LiveVoiceDurationSeconds>,
    },
    Disputed {
        last_valid_seconds: Option<LiveVoiceDurationSeconds>,
        reason: LiveUsageDispute,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum LiveUsageDispute {
    Regression,
    InvalidDuration,
    ConflictingFinal,
    MissingFinal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("live cumulative duration must be finite and nonnegative")]
pub struct LiveVoiceDurationError;
