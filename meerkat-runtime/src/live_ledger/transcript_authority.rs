//! Catalog-owned independent observation coverage.

pub mod dsl;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::live_ledger) mod provider_control;
#[cfg(not(target_arch = "wasm32"))]
pub(in crate::live_ledger) mod voice_usage;

#[cfg(all(not(target_arch = "wasm32"), feature = "live"))]
mod host;
#[cfg(not(target_arch = "wasm32"))]
mod store;
#[cfg(not(target_arch = "wasm32"))]
pub use crate::live_ledger::authority::store::source_reservation::{
    LiveSourceReservationError, LiveSourceReservationOutcome,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "live"))]
pub use host::LiveContinuousChannel;
#[cfg(not(target_arch = "wasm32"))]
pub use provider_control::{CommittedLiveProviderControl, LiveProviderControlOutcome};
#[cfg(not(target_arch = "wasm32"))]
pub use store::{
    CommittedLiveObservation, CommittedLiveVoiceUsage, LiveClientDelegationError,
    LiveClientDelegationOutcome, LiveTranscriptChannelIngress, LiveTranscriptStoreOwner,
    LiveTranscriptWriteError,
};
