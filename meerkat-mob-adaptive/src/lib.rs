//! Compatibility crate for the mob-owned adaptive implementation.
//!
//! Adaptive execution is owned by `meerkat-mob`; this crate remains only as a
//! transitional import path for internal callers while the workspace finishes
//! converging on the mob-owned module.

pub use meerkat_mob::adaptive::*;
