//! meerkat-rpc - JSON-RPC stdio server for Meerkat
//!
//! Provides a JSON-RPC 2.0 stdio interface for IDE integration,
//! desktop apps, and automation tools.

pub mod error;
pub mod handlers;
pub mod protocol;
pub mod router;
pub mod server;
pub mod session_runtime;
pub mod transport;

pub use server::{serve_stdio, serve_stdio_with_skill_runtime};

/// Default capacity for notification / event channels throughout the crate.
pub const NOTIFICATION_CHANNEL_CAPACITY: usize = 256;
