//! wasm32-safe time types.
//!
//! On native: re-exports `std::time::{Instant, SystemTime}`.
//! On wasm32: re-exports `web_time::{Instant, SystemTime}` which use
//! `performance.now()` and `Date.now()` respectively.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Instant, SystemTime};

#[cfg(target_arch = "wasm32")]
pub use web_time::{Instant, SystemTime};

// Duration and UNIX_EPOCH are the same on both
pub use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::UNIX_EPOCH;

#[cfg(target_arch = "wasm32")]
pub use web_time::UNIX_EPOCH;
