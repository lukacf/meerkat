//! meerkat-rpc - JSON-RPC stdio server for Meerkat
//!
//! Provides a JSON-RPC 2.0 stdio interface for IDE integration,
//! desktop apps, and automation tools.

// Keep the lib-unit harness focused on the protocol/error contract tests that
// Phase 0 smokes directly. The heavier router/runtime behavior suites still run
// through integration-test targets and the phase verification commands.
#[cfg(not(test))]
pub mod callback_dispatcher;
pub mod error;
#[cfg(not(test))]
pub mod handlers;
pub mod protocol;
#[cfg(not(test))]
pub mod router;
#[cfg(not(test))]
pub mod server;
#[cfg(not(test))]
pub mod session_executor;
#[cfg(not(test))]
pub mod session_runtime;
#[cfg(not(test))]
pub mod transport;

#[cfg(not(test))]
pub use server::{serve_stdio, serve_stdio_with_skill_runtime};

/// Default capacity for notification / event channels throughout the crate.
pub const NOTIFICATION_CHANNEL_CAPACITY: usize = 256;
