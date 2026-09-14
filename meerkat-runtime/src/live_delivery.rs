//! Public Live delivery facts for generated request authority.
//!
//! These are persisted vocabulary, not a handwritten reducer. They do not
//! classify work or grant another provider write. Public delivery deliberately
//! has no provider-consumed or playback-completed success state.

use serde::{Deserialize, Serialize};

macro_rules! live_delivery_states {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

live_delivery_states! { LiveResultDeliveryState {
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
}}

live_delivery_states! { LiveContinuationBatchEligibility {
    PendingOutputs,
    EligibleUnclaimed,
    ClaimedByAttempt,
    Spent,
    Abandoned,
}}

live_delivery_states! { LiveContinuationState {
    NotClaimed,
    Claimed,
    NotEnqueued,
    WrittenConsumptionUnconfirmed,
    AmbiguousUnfenced,
    AmbiguousFenced,
    NotAttemptedMissingRequiredOutputs,
    AbandonedByClose,
}}

live_delivery_states! { LiveContextChunkDeliveryState {
    NotClaimed,
    Claimed,
    NotEnqueued,
    WrittenInjectionUnconfirmed,
    CorrelatedInjectionObserved,
    AmbiguousUnfenced,
    AmbiguousFenced,
    AbandonedBeforeWrite,
}}

/// An uncorrelated acknowledgment cannot identify a context chunk or advance
/// its correlated-injection frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveContextAcknowledgmentAttribution {
    Uncorrelated {},
    Correlated { client_event_id: String },
}
