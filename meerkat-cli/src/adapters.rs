//! Adapters to bridge existing crate types to agent traits
//!
//! `McpRouterAdapter` has been relocated to `meerkat-mcp`.
//! This module re-exports it for backwards compatibility.

#[cfg(feature = "mcp")]
pub use meerkat_mcp::McpRouterAdapter;
