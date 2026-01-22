//! raik-mcp-client - MCP client for RAIK
//!
//! Connect to MCP servers and route tool calls.

mod error;
mod router;
mod connection;
mod transport;

pub use error::McpError;
pub use router::McpRouter;
pub use connection::McpConnection;

// Re-export McpServerConfig from raik-core for backwards compatibility
pub use raik_core::McpServerConfig;
