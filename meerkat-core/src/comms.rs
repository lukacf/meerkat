//! Canonical communication API types for Meerkat.
//!
//! This module defines the public contract for comms command, response, and stream
//! controls. It intentionally stays transport-agnostic and keeps names stable for
//! the host and SDK surface migration work.

use crate::event::AgentEvent;
use crate::interaction::{InteractionId, ResponseStatus};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerName(pub String);

impl PeerName {
    /// Create a new peer name if it passes basic validation.
    pub fn new(name: impl Into<String>) -> Result<Self, String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("peer name cannot be empty".to_string());
        }
        if name.chars().any(|c| c.is_control()) {
            return Err("peer name cannot contain control characters".to_string());
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for PeerName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for PeerName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<PeerName> for String {
    fn from(peer_name: PeerName) -> Self {
        peer_name.0
    }
}

/// Source for an input event posted to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSource {
    Tcp,
    Uds,
    Stdin,
    Webhook,
    Rpc,
}

impl From<crate::config::PlainEventSource> for InputSource {
    fn from(source: crate::config::PlainEventSource) -> Self {
        match source {
            crate::config::PlainEventSource::Tcp => Self::Tcp,
            crate::config::PlainEventSource::Uds => Self::Uds,
            crate::config::PlainEventSource::Stdin => Self::Stdin,
            crate::config::PlainEventSource::Webhook => Self::Webhook,
            crate::config::PlainEventSource::Rpc => Self::Rpc,
        }
    }
}

impl From<InputSource> for crate::config::PlainEventSource {
    fn from(source: InputSource) -> Self {
        match source {
            InputSource::Tcp => Self::Tcp,
            InputSource::Uds => Self::Uds,
            InputSource::Stdin => Self::Stdin,
            InputSource::Webhook => Self::Webhook,
            InputSource::Rpc => Self::Rpc,
        }
    }
}

/// Whether this input/peer command should reserve a local interaction stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputStreamMode {
    /// Do not reserve any stream.
    None,
    /// Reserve an interaction stream for the command.
    ReserveInteraction,
}

/// Transport-independent comms command envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommsCommand {
    /// Inject input into the local session.
    Input {
        session_id: crate::types::SessionId,
        body: String,
        source: InputSource,
        stream: InputStreamMode,
        allow_self_session: bool,
    },
    /// Send a one-way peer message.
    PeerMessage { to: PeerName, body: String },
    /// Send a request to a peer.
    PeerRequest {
        to: PeerName,
        intent: String,
        params: serde_json::Value,
        stream: InputStreamMode,
    },
    /// Send a response to a prior peer request.
    PeerResponse {
        to: PeerName,
        in_reply_to: InteractionId,
        status: ResponseStatus,
        result: serde_json::Value,
    },
}

impl CommsCommand {
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::Input { .. } => "input",
            Self::PeerMessage { .. } => "peer_message",
            Self::PeerRequest { .. } => "peer_request",
            Self::PeerResponse { .. } => "peer_response",
        }
    }
}

/// Receipt returned after accepting a comms command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendReceipt {
    InputAccepted {
        interaction_id: InteractionId,
        stream_reserved: bool,
    },
    PeerMessageSent {
        envelope_id: uuid::Uuid,
        acked: bool,
    },
    PeerRequestSent {
        envelope_id: uuid::Uuid,
        interaction_id: InteractionId,
        stream_reserved: bool,
    },
    PeerResponseSent {
        envelope_id: uuid::Uuid,
        in_reply_to: InteractionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerDirectorySource {
    Trusted,
    Inproc,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDirectoryEntry {
    pub name: PeerName,
    pub peer_id: String,
    pub address: String,
    pub source: PeerDirectorySource,
    pub sendable_kinds: Vec<String>,
    pub capabilities: serde_json::Value,
}

/// Scope for streaming event output.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StreamScope {
    Session(crate::types::SessionId),
    Interaction(InteractionId),
}

/// Typed stream over agent events.
pub type EventStream = Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;

/// Errors for stream attachment and lookup.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum StreamError {
    #[error("interaction not reserved: {0}")]
    NotReserved(InteractionId),
    #[error("stream not found: {0}")]
    NotFound(String),
    #[error("already attached: {0}")]
    AlreadyAttached(InteractionId),
    #[error("stream closed")]
    Closed,
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("internal: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SendError {
    #[error("peer not found: {0}")]
    PeerNotFound(String),
    #[error("peer offline")]
    PeerOffline,
    #[error("peer not sendable")]
    PeerNotSendable(String),
    #[error("input stream closed")]
    InputClosed,
    #[error("unsupported command: {0}")]
    Unsupported(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("internal: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SendAndStreamError {
    #[error("send failed: {0}")]
    Send(#[from] SendError),
    #[error("stream attach failed: receipt={receipt:?}, error={error}")]
    StreamAttach {
        receipt: SendReceipt,
        error: StreamError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn peer_name_validation() {
        assert!(PeerName::new("alice").is_ok());
        assert!(PeerName::new("".to_string()).is_err());
        assert!(PeerName::new("bad\x00name").is_err());
    }

    #[test]
    fn input_stream_mode_roundtrip() {
        let mode = InputStreamMode::ReserveInteraction;
        let serialized = serde_json::to_value(&mode).unwrap();
        assert_eq!(serialized.as_str(), Some("reserve_interaction"));
        assert_eq!(
            serde_json::from_value::<InputStreamMode>(serialized).unwrap(),
            mode
        );
    }

    #[test]
    fn peer_directory_entry_fields() {
        let entry = PeerDirectoryEntry {
            name: PeerName::new("agent").unwrap(),
            peer_id: "ed25519:abc".to_string(),
            address: "inproc://agent".to_string(),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec!["peer_message".to_string()],
            capabilities: Value::Object(Default::default()),
        };
        assert_eq!(entry.name.0, "agent");
        assert_eq!(entry.source, PeerDirectorySource::Inproc);
    }
}
