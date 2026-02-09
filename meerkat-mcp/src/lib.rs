//! meerkat-mcp - MCP client for Meerkat
//!
//! Connect to MCP servers and route tool calls.

mod adapter;
mod connection;
mod error;
mod protocol;
mod router;
mod transport;

pub use adapter::McpRouterAdapter;
pub use connection::McpConnection;
pub use error::McpError;
pub use protocol::McpProtocol;
pub use router::McpRouter;

// Re-export McpServerConfig from meerkat-core for backwards compatibility
pub use meerkat_core::McpServerConfig;
