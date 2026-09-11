//! Continuous Live transcript observations, distinct from finalized turns.
//!
//! These values carry content, not permission or persistence authority.
//! The feature-owned ingress and wire codec enforce byte/count budgets before
//! accepting an observation into the canonical live ledger.

use std::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

/// A Meerkat-assigned ordinal within one session's committed Live ledger.
///
/// This is never a provider event, response, item, or turn identifier.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveObservationSeq(NonZeroU64);

impl LiveObservationSeq {
    pub fn new(value: u64) -> Result<Self, LiveObservationValueError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(LiveObservationValueError::ZeroSequence)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for LiveObservationSeq {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Direction of observed speech, not a completed transcript role.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveTranscriptDirection {
    Input,
    Output,
}

/// Finite session-relative half-open interval in milliseconds.
///
/// Fractional values, overlaps with other observations, and zero-width
/// intervals are retained. An interval proves neither a turn boundary nor
/// playback completion.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LiveTranscriptRangeWire")]
pub struct LiveTranscriptRange {
    start_ms: f64,
    end_ms: f64,
}

// Construction rejects NaN, so the derived equality remains reflexive.
impl Eq for LiveTranscriptRange {}

impl LiveTranscriptRange {
    pub fn new(start_ms: f64, end_ms: f64) -> Result<Self, LiveObservationValueError> {
        if !start_ms.is_finite() || !end_ms.is_finite() {
            return Err(LiveObservationValueError::NonFiniteRange);
        }
        if start_ms < 0.0 || end_ms < start_ms {
            return Err(LiveObservationValueError::InvalidRange);
        }
        Ok(Self { start_ms, end_ms })
    }

    #[must_use]
    pub const fn start_ms(self) -> f64 {
        self.start_ms
    }

    #[must_use]
    pub const fn end_ms(self) -> f64 {
        self.end_ms
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveTranscriptRangeWire {
    #[cfg_attr(feature = "schema", schemars(range(min = 0.0)))]
    start_ms: f64,
    #[cfg_attr(feature = "schema", schemars(range(min = 0.0)))]
    end_ms: f64,
}

impl TryFrom<LiveTranscriptRangeWire> for LiveTranscriptRange {
    type Error = LiveObservationValueError;

    fn try_from(value: LiveTranscriptRangeWire) -> Result<Self, Self::Error> {
        Self::new(value.start_ms, value.end_ms)
    }
}

/// Exact observed transcript text before canonical sequence assignment.
///
/// Whitespace and empty deltas are content facts. Request construction and
/// permission are separate owners and must not turn this value into a final
/// user transcript or an executable task by itself.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveTranscriptObservation {
    direction: LiveTranscriptDirection,
    range: LiveTranscriptRange,
    text: Box<str>,
}

impl LiveTranscriptObservation {
    #[must_use]
    pub fn new(
        direction: LiveTranscriptDirection,
        range: LiveTranscriptRange,
        text: impl Into<Box<str>>,
    ) -> Self {
        Self {
            direction,
            range,
            text: text.into(),
        }
    }

    #[must_use]
    pub const fn direction(&self) -> LiveTranscriptDirection {
        self.direction
    }

    #[must_use]
    pub const fn range(&self) -> LiveTranscriptRange {
        self.range
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl fmt::Debug for LiveTranscriptObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveTranscriptObservation")
            .field("direction", &self.direction)
            .field("range", &self.range)
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// Invalid observation vocabulary; values are omitted from diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveObservationValueError {
    #[error("a committed live observation sequence must be positive")]
    ZeroSequence,
    #[error("live observation timestamps must be finite")]
    NonFiniteRange,
    #[error("live observation range must be nonnegative and ordered")]
    InvalidRange,
}
