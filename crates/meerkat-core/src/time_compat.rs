//! wasm32-safe time types.
//!
//! On native: re-exports `std::time::{Instant, SystemTime}`.
//! On wasm32: re-exports `web_time::{Instant, SystemTime}` which use
//! `performance.now()` and `Date.now()` respectively.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Instant, SystemTime};

#[cfg(target_arch = "wasm32")]
pub use web_time::{Instant, SystemTime};

/// Monotonic instant for budget horizons and LLM call deadlines.
///
/// On native this is Tokio's instant. In production, and outside any runtime,
/// it reads the same monotonic clock as `std::time::Instant`. Under a paused
/// Tokio clock (`tokio::time::pause`, tests only) it reads the paused clock,
/// so a deadline test controls time exactly instead of racing the machine.
/// wasm32 keeps `web_time`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type DeadlineInstant = tokio::time::Instant;

#[cfg(target_arch = "wasm32")]
pub(crate) type DeadlineInstant = web_time::Instant;

/// Cancellable host timers for the shared WASM Tokio compatibility surface.
#[cfg(target_arch = "wasm32")]
pub mod wasm;

// Duration and UNIX_EPOCH are the same on both
pub use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::UNIX_EPOCH;

#[cfg(target_arch = "wasm32")]
pub use web_time::UNIX_EPOCH;

/// Mint a fresh UUID without relying on unsupported wasm system time.
///
/// Native builds keep UUIDv7 time ordering. Browser/WASM builds use UUIDv4
/// because `uuid::Uuid::now_v7()` currently reaches `std::time` and panics
/// under `wasm32-unknown-unknown`.
pub fn new_uuid_v7() -> uuid::Uuid {
    #[cfg(not(target_arch = "wasm32"))]
    {
        uuid::Uuid::now_v7()
    }
    #[cfg(target_arch = "wasm32")]
    {
        uuid::Uuid::new_v4()
    }
}
