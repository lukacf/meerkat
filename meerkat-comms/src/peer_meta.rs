//! Re-export [`PeerMeta`] from `meerkat-core`.
//!
//! The canonical definition lives in `meerkat_core::peer_meta` so that
//! crates without a `meerkat-comms` dependency (e.g. `meerkat-contracts`)
//! can reference it. This module re-exports everything for convenience.

pub use meerkat_core::peer_meta::PeerMeta;
