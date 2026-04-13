//! Compatibility re-export shim for the renamed Meerkat authority surface.
//!
//! The runtime authority now lives in `crate::meerkat_machine`; this module
//! remains only so older imports and file-path-based tooling do not break
//! during the v1 cutover.

pub use crate::meerkat_machine::*;
