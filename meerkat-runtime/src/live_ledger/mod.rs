//! Independent Live ledger representation, separate from actor Session state.
//!
//! Persisted records are content. Only the store's atomic transaction and
//! generated transition can establish currentness or execution authority.

pub mod attempt;
pub mod completion;
pub mod completion_budget;
pub mod context;
pub mod record;
pub mod source;
pub mod transcript;
pub mod write;
