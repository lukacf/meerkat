//! Public Live delivery facts for generated request authority.
//!
//! These are persisted vocabulary, not a handwritten reducer. They do not
//! classify work or grant another provider write. Public delivery deliberately
//! has no provider-consumed or playback-completed success state.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveResultDeliveryState {
    Authorized,
    Claimed,
    NotEnqueued,
    WrittenConsumptionUnconfirmed,
    AmbiguousUnfenced,
    AmbiguousFenced,
    NotSent,
    RejectedAfterWrite,
    AbandonedByClose,
    AbandonedByReplacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveContinuationBatchEligibility {
    PendingOutputs,
    EligibleUnclaimed,
    ClaimedByAttempt,
    Spent,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveContinuationState {
    NotClaimed,
    Claimed,
    NotEnqueued,
    WrittenConsumptionUnconfirmed,
    AmbiguousUnfenced,
    AmbiguousFenced,
    NotAttemptedMissingRequiredOutputs,
    AbandonedByClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextChunkDeliveryState {
    NotClaimed,
    Claimed,
    NotEnqueued,
    WrittenInjectionUnconfirmed,
    CorrelatedInjectionObserved,
    AmbiguousUnfenced,
    AmbiguousFenced,
    AbandonedBeforeWrite,
}

/// An uncorrelated acknowledgment cannot identify a context chunk or advance
/// its correlated-injection frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveContextAcknowledgmentAttribution {
    Uncorrelated {},
    Correlated { client_event_id: String },
}
