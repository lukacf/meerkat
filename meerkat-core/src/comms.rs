//! Canonical communication API types for Meerkat.
//!
//! This module defines the public contract for comms command, response, and stream
//! controls. It intentionally stays transport-agnostic and keeps names stable for
//! the host and SDK surface migration work.

use crate::event::AgentEvent;
use crate::interaction::{InteractionId, ResponseStatus};
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::pin::Pin;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerName(String);

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

    pub fn as_string(&self) -> String {
        self.0.clone()
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

/// Build inputs for canonical comms command parsing.
#[derive(Debug, Clone, Default)]
pub struct CommsCommandRequest {
    pub kind: String,
    pub to: Option<String>,
    pub body: Option<String>,
    pub intent: Option<String>,
    pub params: Option<serde_json::Value>,
    pub in_reply_to: Option<String>,
    pub status: Option<String>,
    pub result: Option<serde_json::Value>,
    pub source: Option<String>,
    pub stream: Option<String>,
    pub allow_self_session: Option<bool>,
}

/// Validation failure for one command field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommsCommandValidationError {
    pub field: String,
    pub issue: String,
    pub got: Option<String>,
}

impl CommsCommandValidationError {
    fn new(field: impl Into<String>, issue: impl Into<String>, got: Option<String>) -> Self {
        Self {
            field: field.into(),
            issue: issue.into(),
            got,
        }
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        match &self.got {
            Some(got) => json!({
                "field": self.field,
                "issue": self.issue,
                "got": got,
            }),
            None => json!({
                "field": self.field,
                "issue": self.issue,
            }),
        }
    }
}

impl CommsCommandRequest {
    pub fn parse(
        &self,
        session_id: &crate::types::SessionId,
    ) -> Result<CommsCommand, Vec<CommsCommandValidationError>> {
        let mut errors = Vec::new();

        let kind = self.kind.as_str();
        match kind {
            "input" => {
                let Some(body) = self.body.clone() else {
                    errors.push(CommsCommandValidationError::new(
                        "body",
                        "required_field",
                        None,
                    ));
                    return Err(errors);
                };

                let source = match self.source.as_deref() {
                    Some("tcp") => InputSource::Tcp,
                    Some("uds") => InputSource::Uds,
                    Some("stdin") => InputSource::Stdin,
                    Some("webhook") => InputSource::Webhook,
                    Some("rpc") | None => InputSource::Rpc,
                    Some(other) => {
                        errors.push(CommsCommandValidationError::new(
                            "source",
                            "invalid_value",
                            Some(other.to_string()),
                        ));
                        return Err(errors);
                    }
                };

                let stream = match self.stream.as_deref() {
                    Some("reserve_interaction") => InputStreamMode::ReserveInteraction,
                    Some("none") | None => InputStreamMode::None,
                    Some(other) => {
                        errors.push(CommsCommandValidationError::new(
                            "stream",
                            "invalid_value",
                            Some(other.to_string()),
                        ));
                        return Err(errors);
                    }
                };

                Ok(CommsCommand::Input {
                    session_id: session_id.clone(),
                    body,
                    source,
                    stream,
                    allow_self_session: self.allow_self_session.unwrap_or(false),
                })
            }
            "peer_message" => {
                let to = to_peer_name(&self.to, &mut errors);
                if let Some(to) = to {
                    Ok(CommsCommand::PeerMessage {
                        to,
                        body: self.body.clone().unwrap_or_default(),
                    })
                } else {
                    Err(errors)
                }
            }
            "peer_request" => {
                let to = to_peer_name(&self.to, &mut errors);
                let intent = match self.intent.clone() {
                    Some(intent) => intent,
                    None => {
                        errors.push(CommsCommandValidationError::new(
                            "intent",
                            "required_field",
                            None,
                        ));
                        return Err(errors);
                    }
                };
                let stream = match self.stream.as_deref() {
                    Some("reserve_interaction") => InputStreamMode::ReserveInteraction,
                    Some("none") | None => InputStreamMode::None,
                    Some(other) => {
                        errors.push(CommsCommandValidationError::new(
                            "stream",
                            "invalid_value",
                            Some(other.to_string()),
                        ));
                        return Err(errors);
                    }
                };
                if errors.is_empty() {
                    let to = match to {
                        Some(to) => to,
                        None => {
                            return Err(errors);
                        }
                    };
                    Ok(CommsCommand::PeerRequest {
                        to,
                        intent,
                        params: self.params.clone().unwrap_or_default(),
                        stream,
                    })
                } else {
                    Err(errors)
                }
            }
            "peer_response" => {
                let to = to_peer_name(&self.to, &mut errors);
                let in_reply_to = match &self.in_reply_to {
                    Some(in_reply_to) => match uuid::Uuid::parse_str(in_reply_to) {
                        Ok(id) => crate::interaction::InteractionId(id),
                        Err(_) => {
                            errors.push(CommsCommandValidationError::new(
                                "in_reply_to",
                                "invalid_uuid",
                                Some(in_reply_to.clone()),
                            ));
                            return Err(errors);
                        }
                    },
                    None => {
                        errors.push(CommsCommandValidationError::new(
                            "in_reply_to",
                            "required_field",
                            None,
                        ));
                        return Err(errors);
                    }
                };
                let status = match self.status.as_deref() {
                    Some("accepted") => crate::ResponseStatus::Accepted,
                    Some("completed") | None => crate::ResponseStatus::Completed,
                    Some("failed") => crate::ResponseStatus::Failed,
                    Some(other) => {
                        errors.push(CommsCommandValidationError::new(
                            "status",
                            "invalid_value",
                            Some(other.to_string()),
                        ));
                        return Err(errors);
                    }
                };
                if errors.is_empty() {
                    let to = match to {
                        Some(to) => to,
                        None => {
                            return Err(errors);
                        }
                    };
                    Ok(CommsCommand::PeerResponse {
                        to,
                        in_reply_to,
                        status,
                        result: self.result.clone().unwrap_or(serde_json::Value::Null),
                    })
                } else {
                    Err(errors)
                }
            }
            other => {
                errors.push(CommsCommandValidationError::new(
                    "kind",
                    "unknown_kind",
                    Some(other.to_string()),
                ));
                Err(errors)
            }
        }
    }

    pub fn validation_errors_to_json(
        errors: &[CommsCommandValidationError],
    ) -> Vec<serde_json::Value> {
        errors
            .iter()
            .map(CommsCommandValidationError::to_json_value)
            .collect()
    }
}

fn to_peer_name(
    value: &Option<String>,
    errors: &mut Vec<CommsCommandValidationError>,
) -> Option<PeerName> {
    match value {
        Some(name) => match PeerName::new(name) {
            Ok(peer) => Some(peer),
            Err(_) => {
                errors.push(CommsCommandValidationError::new(
                    "to",
                    "invalid_value",
                    Some(name.clone()),
                ));
                None
            }
        },
        None => {
            errors.push(CommsCommandValidationError::new(
                "to",
                "required_field",
                None,
            ));
            None
        }
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
    TrustedAndInproc,
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
    /// Supplementary discovery metadata (description, labels).
    pub meta: crate::PeerMeta,
}

/// Canonical payload for registering a trusted peer through a runtime seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeerSpec {
    pub name: String,
    pub peer_id: String,
    pub address: String,
}

impl TrustedPeerSpec {
    pub fn new(
        name: impl Into<String>,
        peer_id: impl Into<String>,
        address: impl Into<String>,
    ) -> Result<Self, String> {
        let name = PeerName::new(name.into())?;
        Ok(Self {
            name: name.0,
            peer_id: peer_id.into(),
            address: address.into(),
        })
    }
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
    fn input_stream_mode_roundtrip() -> Result<(), serde_json::Error> {
        let mode = InputStreamMode::ReserveInteraction;
        let serialized = serde_json::to_value(mode)?;
        assert_eq!(serialized.as_str(), Some("reserve_interaction"));
        assert_eq!(serde_json::from_value::<InputStreamMode>(serialized)?, mode);
        Ok(())
    }

    #[test]
    fn peer_directory_entry_fields() -> Result<(), String> {
        let entry = PeerDirectoryEntry {
            name: PeerName::new("agent")?,
            peer_id: "ed25519:abc".to_string(),
            address: "inproc://agent".to_string(),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec!["peer_message".to_string()],
            capabilities: Value::Object(Default::default()),
            meta: crate::PeerMeta::default(),
        };
        assert_eq!(entry.name.as_str(), "agent");
        assert_eq!(entry.source, PeerDirectorySource::Inproc);
        Ok(())
    }

    #[test]
    fn trusted_peer_spec_requires_valid_name() {
        let invalid = TrustedPeerSpec::new("", "ed25519:abc", "inproc://a");
        assert!(invalid.is_err());
    }

    #[test]
    fn trusted_peer_spec_keeps_peer_id_and_address() -> Result<(), String> {
        let spec = TrustedPeerSpec::new("alice", "ed25519:abc", "inproc://alice")?;
        assert_eq!(spec.name, "alice");
        assert_eq!(spec.peer_id, "ed25519:abc");
        assert_eq!(spec.address, "inproc://alice");
        Ok(())
    }
}
