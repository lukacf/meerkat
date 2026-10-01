//! Process-local grant issuance, revocation and exact lineage resolution.
//!
//! The generated grant owner is the sole mutable grant authority. The embedding
//! supplies trusted configuration, authenticated callers and complete native
//! custody for revocation. Resolved grants remain data to conjoin with current
//! policy, not entry permits. This crate does not enable model/tool checks,
//! native admission, persistence, consent or operating-system confinement.

pub mod clock;
pub mod grants;
pub mod publication;
