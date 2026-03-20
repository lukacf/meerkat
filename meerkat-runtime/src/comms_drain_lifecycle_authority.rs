//! Shared comms drain lifecycle authority.
//!
//! The canonical implementation now lives in `meerkat-core` so both
//! direct `SessionService` host mode and runtime-backed drain ownership
//! realize the same lifecycle truth.

pub use meerkat_core::comms_drain_lifecycle_authority::*;
