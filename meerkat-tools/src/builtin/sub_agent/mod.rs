//! Sub-agent tools for hierarchical agent workflows
//!
//! This module provides tools that allow LLM agents to spawn and manage sub-agents,
//! enabling hierarchical agent workflows. Sub-agents can use different providers/models
//! (e.g., Claude spawning Gemini/GPT).
//!
//! ## Key Semantics
//!
//! - **Spawn**: Create sub-agent with clean context (just the prompt)
//! - **Fork**: Create sub-agent with continued context (inherits full conversation history)
//!
//! ## Available Tools
//!
//! - `agent_spawn`: Create sub-agent with clean context
//! - `agent_fork`: Clone current agent with full history
//! - `agent_status`: Get status/output of sub-agent by ID
//! - `agent_cancel`: Cancel a running sub-agent
//! - `agent_list`: List all sub-agents with their states
//!
//! Note: For sending messages to sub-agents, use `comms_send` instead of a dedicated steer tool.

mod config;
mod runner;
mod state;

mod cancel;
mod fork;
mod list;
mod spawn;
mod status;
mod tool_set;

pub use config::{SubAgentConfig, SubAgentError};
pub use meerkat_core::ParentCommsContext;
pub use runner::{
    DynLlmClientAdapter, DynSubAgentSpec, LlmClientAdapter, SubAgentCommsConfig, SubAgentHandle,
    SubAgentRunnerError, SubAgentSpec, create_child_comms_config, create_child_peer_entry,
    create_child_trusted_peers, create_fork_session, create_spawn_session, setup_child_comms,
    spawn_sub_agent, spawn_sub_agent_dyn,
};
pub use state::SubAgentToolState;
pub use tool_set::SubAgentToolSet;

pub use cancel::AgentCancelTool;
pub use fork::AgentForkTool;
pub use list::AgentListTool;
pub use spawn::AgentSpawnTool;
pub use status::AgentStatusTool;
