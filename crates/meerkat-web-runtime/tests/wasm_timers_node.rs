#![cfg(target_arch = "wasm32")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

// The same ownership assertions run with Node's object timer handles and the
// browser's integer handles. Keep this binary in the default Node test mode.
#[path = "support/wasm_timers.rs"]
mod tests;
