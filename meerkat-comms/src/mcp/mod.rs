//! MCP tool implementations for Meerkat comms.

pub mod tools;

pub use tools::{
    ListPeersInput, SendMessageInput, SendRequestInput, SendResponseInput, ToolContext,
    handle_tools_call, tools_list,
};
