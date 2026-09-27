#![cfg(target_arch = "wasm32")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

#[path = "support/wasm_timers.rs"]
mod tests;
