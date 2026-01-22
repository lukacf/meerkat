//! meerkat-comms-mcp - MCP tools for Meerkat inter-agent communication
//!
//! This crate provides MCP tools for sending messages between Meerkat instances:
//! - `send_message` - Send a message to a peer
//! - `send_request` - Send a request to a peer
//! - `send_response` - Reply to a request
//! - `list_peers` - List trusted peers

pub mod tools;

pub use tools::{
    handle_tools_call, tools_list, ListPeersInput, SendMessageInput, SendRequestInput,
    SendResponseInput,
};
