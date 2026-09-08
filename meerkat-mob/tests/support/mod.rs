//! Integration-test entry point for the shared multi-host fixtures.
//!
//! Lib tests include `shared.rs` with their own explicit local-crate binding.

use ::meerkat_mob as fixture_mob;

mod shared;
pub use shared::*;
