//! meerkat-rpc - JSON-RPC stdio server for Meerkat
//!
//! Provides a JSON-RPC 2.0 stdio interface for IDE integration,
//! desktop apps, and automation tools.

#![allow(
    dead_code,
    unused_imports,
    clippy::large_futures,
    clippy::redundant_closure_for_method_calls,
    clippy::unnested_or_patterns
)]

// Keep the lib-unit harness focused on the protocol/error contract tests that
// Phase 0 smokes directly. The heavier router/runtime behavior suites still run
// through integration-test targets and the phase verification commands.
pub mod callback_dispatcher;
pub mod error;
pub mod handlers;
pub mod live_projection_sink;
pub mod protocol;
pub mod router;
pub mod secure_rpc;
pub mod server;
pub mod session_executor;
pub mod session_runtime;
pub mod transport;

pub use server::{
    LiveWsConfig, serve_stdio, serve_stdio_with_options, serve_stdio_with_skill_runtime, serve_tcp,
    serve_tcp_connection, serve_tcp_connection_with_options, serve_tcp_with_options,
};

/// Default capacity for notification / event channels throughout the crate.
pub const NOTIFICATION_CHANNEL_CAPACITY: usize = 4096;
