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

pub mod comms_agent;
pub mod listener;
pub mod manager;
pub mod tool_dispatcher;
pub mod types;

pub use comms_agent::{CommsAgent, CommsAgentBuilder};
pub use listener::{spawn_tcp_listener, spawn_uds_listener, ListenerHandle};
pub use manager::{CommsManager, CommsManagerConfig};
pub use tool_dispatcher::CommsToolDispatcher;
pub use types::{CommsContent, CommsMessage};
