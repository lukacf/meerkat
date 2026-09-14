//! Independent Live ledger representation, separate from actor Session state.
//!
//! Persisted records are content. Only the store's atomic transaction and
//! generated transition can establish currentness or execution authority.

pub mod attempt;
pub mod authority;
pub mod completion;
pub mod completion_budget;
pub mod context;
pub mod history;
pub mod record;
pub mod send_attempt;
pub mod source;
pub mod transcript;
pub mod transcript_authority;
pub mod write;

#[cfg(not(target_arch = "wasm32"))]
pub use authority::store::claim::LiveEffectPolicyObservation;
