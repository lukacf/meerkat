//! Self-tests for the `mock_realtime_ws` harness.
//!
//! These keep the harness honest without being a product regression pin —
//! every other consumer of the mock pulls the module in via `#[path]`.

#![cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "mock_realtime_ws/mod.rs"]
mod mock_realtime_ws;
