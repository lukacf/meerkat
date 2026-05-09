//! Live-channel orchestration.
//!
//! Populated by W2-A (precheck / open-config / propagate /
//! materialize-staged) and W2-C (LLM hot-swap surface).
//!
//! Gated by the `live` feature on the `meerkat` facade so surfaces
//! that don't ship a live channel (CLI today, MCP-server, embedded
//! examples) don't pull in the `meerkat-live` dependency.
#![cfg(feature = "live")]
