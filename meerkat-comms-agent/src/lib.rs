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
pub mod types;

pub use comms_agent::{CommsAgent, CommsAgentBuilder};
pub use listener::{ListenerHandle, spawn_tcp_listener, spawn_uds_listener};
pub use manager::{CommsManager, CommsManagerConfig};
pub use meerkat_tools::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
pub use types::{CommsContent, CommsMessage};

// wrap_with_comms is re-exported from meerkat-tools
