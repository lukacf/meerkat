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
pub use router::{
    McpApplyDelta, McpLifecycleAction, McpReloadTarget, McpRouter, McpServerLifecycleState,
};

// Re-export McpServerConfig from meerkat-core for backwards compatibility
pub use meerkat_core::McpServerConfig;

// Skill registration
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "mcp-server-setup",
        name: "MCP Server Setup",
        description: "How to configure MCP servers in .rkat/mcp.toml",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &[],
        body: include_str!("../skills/mcp-server-setup/SKILL.md"),
        extensions: &[],
    }
}
