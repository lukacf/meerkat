//! External-crate entry point for standalone CLI/raw-comms fixture consumers.

pub(crate) use ::meerkat_mob;

#[path = "probe_shared.rs"]
mod shared;
pub use shared::*;
