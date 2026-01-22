//! meerkat-mcp-client - MCP client for Meerkat
//!
//! Connect to MCP servers and route tool calls.

mod error;
mod router;
mod connection;
mod transport;

pub use error::McpError;
pub use router::McpRouter;
pub use connection::McpConnection;

// Re-export McpServerConfig from meerkat-core for backwards compatibility
pub use meerkat_core::McpServerConfig;
