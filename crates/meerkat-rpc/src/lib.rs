//! meerkat-rpc - JSON-RPC server for Meerkat
//!
//! Provides JSON-RPC 2.0 over stdio by default, with an opt-in TCP listener
//! for trusted host integrations. The same method router backs IDEs, desktop
//! apps, SDK subprocesses, and automation tools.

#![allow(
    dead_code,
    unused_imports,
    clippy::large_futures,
    clippy::redundant_closure_for_method_calls,
    clippy::unnested_or_patterns
)]

// Test-feature canary. This crate's tests build with its test features,
// which no dev-dependency turns on: in this repository every test command
// passes them (`node scripts/core-workspace-args.mjs --tests -p meerkat-rpc`
// prints them). A test build without them would compile the tests they
// gate out and pass over fewer tests, so it fails to compile here instead.
// Only code generation instantiates the generic function below (a test
// binary does), so `cargo check`, clippy and an editor's check run, which
// generate no code, are not stopped by it.
// `.cargo/config.toml` [env] sets MEERKAT_WORKSPACE_TEST_FEATURES for this
// repository's builds only; the published package builds without it.
#[cfg(all(test, not(feature = "workspace-tests")))]
mod test_features_canary {
    fn require<T>() {
        const {
            let _ = size_of::<T>();
            assert!(
                option_env!("MEERKAT_WORKSPACE_TEST_FEATURES").is_none(),
                "meerkat-rpc: this repository's test builds need the crate's test features: \
                 `cargo test -p meerkat-rpc --features workspace-tests` \
                 (core-workspace-args.mjs --tests prints the flags for any selection)"
            );
        }
    }

    #[used]
    static REQUIRE: fn() = require::<()>;
}

// Keep the lib-unit harness focused on the protocol/error contract tests that
// Phase 0 smokes directly. The heavier router/runtime behavior suites still run
// through integration-test targets and the phase verification commands.
pub mod callback_dispatcher;
#[cfg(feature = "mob")]
pub mod detached_owner;
pub mod error;
#[cfg(feature = "local-authorization")]
pub mod governed_jsonl;
pub mod handlers;
pub mod live_projection_sink;
#[cfg(feature = "openai-realtime")]
pub mod live_wiring;
pub mod protocol;
pub mod router;
pub mod secure_rpc;
pub mod server;
pub mod session_executor;
pub mod session_runtime;
pub mod transport;

pub use server::{
    LiveConfig, LiveWsConfig, serve_stdio, serve_stdio_with_options,
    serve_stdio_with_options_until, serve_stdio_with_skill_runtime, serve_tcp,
    serve_tcp_connection, serve_tcp_connection_with_options, serve_tcp_with_options,
    serve_tcp_with_options_until,
};

/// Default capacity for notification / event channels throughout the crate.
pub const NOTIFICATION_CHANNEL_CAPACITY: usize = 4096;
