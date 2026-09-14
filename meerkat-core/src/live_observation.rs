//! Continuous Live transcript observations, distinct from finalized turns.
//!
//! These values carry content, not permission or persistence authority.
//! The feature-owned ingress and wire codec enforce byte/count budgets before
//! accepting an observation into the canonical live ledger.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

/// One local channel's received-TEXT counter, independent of durable ledger
/// sequence numbers. Clones share the same counter; no receipt is a grant.
#[derive(Clone, Default)]
pub struct LiveObservationReceiveClock(Arc<AtomicU64>);

#[derive(Clone)]
pub struct LiveObservationReceiveReceipt {
    clock: Arc<AtomicU64>,
    ordinal: NonZeroU64,
}

impl LiveObservationReceiveClock {
    pub fn record_received(
        &self,
    ) -> Result<LiveObservationReceiveReceipt, LiveObservationValueError> {
        let previous = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| LiveObservationValueError::ReceiveSequenceExhausted)?;
        let ordinal = previous
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or(LiveObservationValueError::ReceiveSequenceExhausted)?;
        Ok(LiveObservationReceiveReceipt {
            clock: Arc::clone(&self.0),
            ordinal,
        })
    }

    pub fn received_ordinal(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

impl LiveObservationReceiveReceipt {
    pub fn belongs_to(&self, clock: &LiveObservationReceiveClock) -> bool {
        Arc::ptr_eq(&self.clock, &clock.0)
    }

    pub const fn ordinal(&self) -> u64 {
        self.ordinal.get()
    }
}

impl std::fmt::Debug for LiveObservationReceiveClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveObservationReceiveClock")
            .field("received", &self.received_ordinal())
            .finish()
    }
}

impl std::fmt::Debug for LiveObservationReceiveReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveObservationReceiveReceipt")
            .field("ordinal", &self.ordinal)
            .finish_non_exhaustive()
    }
}

impl PartialEq for LiveObservationReceiveReceipt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.clock, &other.clock) && self.ordinal == other.ordinal
    }
}
impl Eq for LiveObservationReceiveReceipt {}

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
    #[error("local Live observation receive sequence is exhausted")]
    ReceiveSequenceExhausted,
    #[error("a committed live observation sequence must be positive")]
    ZeroSequence,
    #[error("live observation timestamps must be finite")]
    NonFiniteRange,
    #[error("live observation range must be nonnegative and ordered")]
    InvalidRange,
}

#[cfg(test)]
mod receive_tests {
    use super::*;

    #[test]
    fn receive_receipts_bind_one_shared_clock_without_reconstructible_ordinals()
    -> Result<(), LiveObservationValueError> {
        let clock = LiveObservationReceiveClock::default();
        let shared = clock.clone();
        let first = clock.record_received()?;
        let second = shared.record_received()?;
        assert_eq!(first.ordinal(), 1);
        assert_eq!(second.ordinal(), 2);
        assert_eq!(clock.received_ordinal(), 2);
        assert!(first.belongs_to(&shared));
        let foreign = LiveObservationReceiveClock::default();
        let foreign_first = foreign.record_received()?;
        assert!(!first.belongs_to(&foreign));
        assert_ne!(first, foreign_first);
        assert_eq!(first, first.clone());
        Ok(())
    }

    #[test]
    fn receive_clock_exhaustion_never_wraps_or_mints_zero() {
        let clock = LiveObservationReceiveClock::default();
        clock.0.store(u64::MAX, Ordering::Release);
        assert_eq!(
            clock.record_received().err(),
            Some(LiveObservationValueError::ReceiveSequenceExhausted)
        );
        assert_eq!(clock.received_ordinal(), u64::MAX);
    }
}
