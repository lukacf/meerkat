//! meerkat-comms-agent - Agent loop integration for Meerkat inter-agent communication
//!
//! This crate provides the integration between meerkat-comms and the Meerkat agent loop.
//! It enables agents to:
//! - Receive messages from other agents via inbox injection
//! - Send messages, requests, and responses to other agents via comms tools
//! - Spawn listeners for incoming connections (UDS and TCP)
//!
//! # Architecture
//!
//! The main components are:
//!
//! - [`CommsMessage`] - Session-injectable message types
//! - [`CommsManager`] - Manages keypair, trusted peers, inbox, and router
//! - [`CommsToolDispatcher`] - Implements `AgentToolDispatcher` for comms tools
//! - [`CommsAgent`] - Wrapper around `Agent` that injects inbox at turn boundaries
//!
//! # Uniform Comms Setup
//!
//! Use [`wrap_with_comms`] to add comms tools to any tool dispatcher. This provides
//! a uniform way to enable comms for both main agents and sub-agents:
//!
//! ```ignore
//! use meerkat_comms_agent::wrap_with_comms;
//!
//! // For main agent
//! let tools_with_comms = wrap_with_comms(tools, &runtime);
//!
//! // For sub-agent (same function)
//! let child_tools_with_comms = wrap_with_comms(child_tools, &child_runtime);
//! ```

pub mod comms_agent;
pub mod listener;
pub mod manager;
pub mod tool_dispatcher;
pub mod types;

use meerkat_core::{AgentToolDispatcher, CommsRuntime};
use std::sync::Arc;

pub use comms_agent::{CommsAgent, CommsAgentBuilder};
pub use listener::{ListenerHandle, spawn_tcp_listener, spawn_uds_listener};
pub use manager::{CommsManager, CommsManagerConfig};
pub use tool_dispatcher::{CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher};
pub use types::{CommsContent, CommsMessage};

/// Wrap a tool dispatcher with comms tools.
///
/// This is the preferred way to add comms capabilities to any agent. It works
/// uniformly for both main agents and sub-agents, treating comms as core
/// infrastructure rather than an optional feature.
///
/// The resulting dispatcher:
/// - Provides comms tools: send_message, send_request, send_response, list_peers
/// - Delegates all other tools to the inner dispatcher
///
/// # Example
///
/// ```ignore
/// // Create base tools (builtin + MCP)
/// let base_tools: Arc<dyn AgentToolDispatcher> = ...;
///
/// // Prepare comms runtime
/// let bootstrap = CommsBootstrap::from_config(config, base_dir);
/// let prepared = bootstrap.prepare().await?.unwrap();
///
/// // Wrap with comms
/// let tools = wrap_with_comms(base_tools, &prepared.runtime);
/// ```
pub fn wrap_with_comms(
    tools: Arc<dyn AgentToolDispatcher>,
    runtime: &CommsRuntime,
) -> Arc<dyn AgentToolDispatcher> {
    let router = runtime.router_arc();
    let trusted_peers = runtime.trusted_peers_shared();
    Arc::new(DynCommsToolDispatcher::new(router, trusted_peers, tools))
}
