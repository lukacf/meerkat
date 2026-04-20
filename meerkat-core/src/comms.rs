//! Canonical communication API types for Meerkat.
//!
//! This module defines the public contract for comms command, response, and stream
//! controls. It intentionally stays transport-agnostic and keeps names stable for
//! the host and SDK surface migration work.

use crate::event::{AgentEvent, EventEnvelope};
use crate::interaction::{InteractionId, ResponseStatus};
use crate::types::{ContentBlock, HandlingMode};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerName(String);

impl PeerName {
    /// Create a new peer name if it passes basic validation.
    pub fn new(name: impl Into<String>) -> Result<Self, String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("peer name cannot be empty".to_string());
        }
        if name.chars().any(char::is_control) {
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

/// Typed wire request for `comms/send`.
///
/// Variants are serde-tagged on `kind` and validated structurally at the
/// deserialization boundary. Required fields per kind are enforced by the
/// type system; invalid discriminators (`source`, `stream`, `handling_mode`,
/// `status`) become serde deserialization errors rather than runtime
/// string-match failures.
///
/// Cross-field invariants that cannot be expressed structurally (e.g.
/// `handling_mode` is forbidden on `Accepted` peer responses) are checked
/// in [`CommsCommandRequest::into_command`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommsCommandRequest {
    /// Inject input into the local session.
    Input {
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<InputSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_self_session: Option<bool>,
    },
    /// Send a one-way peer message.
    PeerMessage {
        to: PeerName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
    /// Send a request to a peer.
    PeerRequest {
        to: PeerName,
        intent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<serde_json::Value>,
        /// Legacy promotion: if `params` is absent, `body` is wrapped as
        /// `{"body": <body>}` for backwards-compatibility with single-string
        /// requesters. Prefer `params`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
    },
    /// Send a response to a prior peer request.
    PeerResponse {
        to: PeerName,
        in_reply_to: InteractionId,
        status: ResponseStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
}

/// Cross-field validation failure for [`CommsCommandRequest::into_command`].
///
/// Per-field discriminator validation is enforced by serde at deserialization
/// — only invariants that span multiple fields surface here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommsCommandError {
    /// `handling_mode` is set on a `peer_response` whose `status` is
    /// `Accepted`. Progress responses cannot carry a handling mode — the
    /// receiver's admission gate would drop them, so reject at parse time.
    #[error("handling_mode is forbidden on accepted peer responses")]
    HandlingModeForbiddenForAcceptedResponse,
}

impl CommsCommandRequest {
    /// Convert the typed wire request into a [`CommsCommand`] domain envelope.
    ///
    /// `session_id` is supplied separately because it is owned by the
    /// surface that received the request, not the wire payload.
    pub fn into_command(
        self,
        session_id: &crate::types::SessionId,
    ) -> Result<CommsCommand, CommsCommandError> {
        Ok(match self {
            CommsCommandRequest::Input {
                body,
                blocks,
                source,
                stream,
                handling_mode,
                allow_self_session,
            } => CommsCommand::Input {
                session_id: session_id.clone(),
                body,
                blocks,
                handling_mode: handling_mode.unwrap_or_default(),
                source: source.unwrap_or(InputSource::Rpc),
                stream: stream.unwrap_or(InputStreamMode::None),
                allow_self_session: allow_self_session.unwrap_or(false),
            },
            CommsCommandRequest::PeerMessage {
                to,
                body,
                blocks,
                handling_mode,
            } => CommsCommand::PeerMessage {
                to,
                body: body.unwrap_or_default(),
                blocks,
                handling_mode: handling_mode.unwrap_or_default(),
            },
            CommsCommandRequest::PeerRequest {
                to,
                intent,
                params,
                body,
                handling_mode,
                stream,
            } => CommsCommand::PeerRequest {
                to,
                intent,
                params: match (params, body) {
                    (Some(p), _) => p,
                    (None, Some(body)) => serde_json::json!({ "body": body }),
                    (None, None) => serde_json::Value::Object(Default::default()),
                },
                handling_mode: handling_mode.unwrap_or_default(),
                stream: stream.unwrap_or(InputStreamMode::None),
            },
            CommsCommandRequest::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                handling_mode,
            } => {
                if status == ResponseStatus::Accepted && handling_mode.is_some() {
                    return Err(CommsCommandError::HandlingModeForbiddenForAcceptedResponse);
                }
                CommsCommand::PeerResponse {
                    to,
                    in_reply_to,
                    status,
                    result: result.unwrap_or(serde_json::Value::Null),
                    handling_mode,
                }
            }
        })
    }

    /// Stable wire discriminant for telemetry / logging.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Input { .. } => "input",
            Self::PeerMessage { .. } => "peer_message",
            Self::PeerRequest { .. } => "peer_request",
            Self::PeerResponse { .. } => "peer_response",
        }
    }
}
/// Source for an input event posted to an agent.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
        blocks: Option<Vec<ContentBlock>>,
        handling_mode: HandlingMode,
        source: InputSource,
        stream: InputStreamMode,
        allow_self_session: bool,
    },
    /// Send a one-way peer message.
    PeerMessage {
        to: PeerName,
        body: String,
        blocks: Option<Vec<ContentBlock>>,
        handling_mode: HandlingMode,
    },
    /// Send a request to a peer.
    PeerRequest {
        to: PeerName,
        intent: String,
        params: serde_json::Value,
        handling_mode: HandlingMode,
        stream: InputStreamMode,
    },
    /// Send a response to a prior peer request.
    PeerResponse {
        to: PeerName,
        in_reply_to: InteractionId,
        status: ResponseStatus,
        result: serde_json::Value,
        handling_mode: Option<HandlingMode>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerReachability {
    Unknown,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PeerReachabilityReason {
    OfflineOrNoAck,
    TransportError,
    /// The peer admitted the transport but rejected our envelope at its
    /// ingress policy gate (untrusted sender, full inbox, etc.). The peer
    /// is still reachable at the transport level; policy denied us.
    AdmissionDropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDirectoryEntry {
    pub name: PeerName,
    pub peer_id: String,
    pub address: String,
    pub source: PeerDirectorySource,
    pub sendable_kinds: Vec<String>,
    pub capabilities: serde_json::Value,
    pub reachability: PeerReachability,
    pub last_unreachable_reason: Option<PeerReachabilityReason>,
    /// Supplementary discovery metadata (description, labels).
    pub meta: crate::PeerMeta,
}

/// Canonical payload for registering a trusted peer through a runtime seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Typed stream over enveloped agent events.
pub type EventStream = Pin<Box<dyn Stream<Item = EventEnvelope<AgentEvent>> + Send>>;

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

/// Typed reason a peer rejected our envelope at its ingress admission gate.
///
/// This mirrors `meerkat_comms::DropReason` across the core boundary so
/// `SendError::AdmissionDropped` can carry the typed cause all the way to
/// REST/RPC/MCP error payloads. Callers distinguish transport-level failure
/// (`PeerOffline`) from policy-level rejection (`AdmissionDropped { reason }`)
/// without collapsing both into "peer unreachable".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdmissionDropReason {
    /// `require_peer_auth` is on, the sender is not in the trusted set, and
    /// the envelope is not auth-exempt (e.g. supervisor-bridge bootstrap).
    UntrustedSender,
    /// Classification rejected the item before the admission gate ran.
    ClassificationRejected,
    /// The receiver's classified inbox is closed (receiver dropped).
    SessionClosed,
    /// The receiver's classified inbox is at capacity.
    InboxFull,
}

impl AdmissionDropReason {
    /// Stable wire code for this drop reason, suitable for REST/RPC/MCP
    /// error payloads. Callers-facing discriminant — must stay stable.
    pub fn as_code(&self) -> &'static str {
        match self {
            AdmissionDropReason::UntrustedSender => "untrusted_sender",
            AdmissionDropReason::ClassificationRejected => "classification_rejected",
            AdmissionDropReason::SessionClosed => "session_closed",
            AdmissionDropReason::InboxFull => "inbox_full",
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
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
    /// Receiver admitted the envelope-transport but rejected it at ingress
    /// for a typed policy reason (untrusted sender, full inbox, etc.). This
    /// is semantically distinct from `PeerOffline` — transport worked,
    /// policy refused.
    #[error("peer dropped at admission: {reason:?}")]
    AdmissionDropped { reason: AdmissionDropReason },
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
    fn peer_directory_entry_fields() -> Result<(), String> {
        let entry = PeerDirectoryEntry {
            name: PeerName::new("agent")?,
            peer_id: "ed25519:abc".to_string(),
            address: "inproc://agent".to_string(),
            source: PeerDirectorySource::Inproc,
            sendable_kinds: vec!["peer_message".to_string()],
            capabilities: Value::Object(serde_json::Map::default()),
            reachability: PeerReachability::Unknown,
            last_unreachable_reason: None,
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

    // -- peer_request body→params promotion tests (PR #156 port) --

    fn peer_request_cmd(
        params: Option<Value>,
        body: Option<String>,
    ) -> Result<CommsCommand, CommsCommandError> {
        let req = CommsCommandRequest::PeerRequest {
            to: PeerName::new("bob").expect("peer name"),
            intent: "greet".to_string(),
            params,
            body,
            handling_mode: None,
            stream: None,
        };
        req.into_command(&crate::types::SessionId::new())
    }

    #[test]
    fn peer_request_with_params_only() {
        let cmd = peer_request_cmd(Some(serde_json::json!({"key": "value"})), None).unwrap();
        match cmd {
            CommsCommand::PeerRequest { params, .. } => {
                assert_eq!(params["key"], "value");
            }
            other => panic!("expected PeerRequest, got {other:?}"),
        }
    }

    #[test]
    fn peer_request_with_body_only_promotes_to_params() {
        let cmd = peer_request_cmd(None, Some("hello world".to_string())).unwrap();
        match cmd {
            CommsCommand::PeerRequest { params, .. } => {
                assert_eq!(
                    params["body"], "hello world",
                    "body should be promoted into params.body when params is absent"
                );
            }
            other => panic!("expected PeerRequest, got {other:?}"),
        }
    }

    #[test]
    fn peer_request_with_both_prefers_params() {
        let cmd = peer_request_cmd(
            Some(serde_json::json!({"explicit": true})),
            Some("ignored body".to_string()),
        )
        .unwrap();
        match cmd {
            CommsCommand::PeerRequest { params, .. } => {
                assert_eq!(params["explicit"], true);
                assert!(
                    params.get("body").is_none(),
                    "body should not be promoted when params is present"
                );
            }
            other => panic!("expected PeerRequest, got {other:?}"),
        }
    }

    #[test]
    fn peer_request_with_neither_gives_empty_object() {
        let cmd = peer_request_cmd(None, None).unwrap();
        match cmd {
            CommsCommand::PeerRequest { params, .. } => {
                assert!(params.is_object(), "params should be an object");
                assert!(
                    params.as_object().unwrap().is_empty(),
                    "params should be empty when both body and params are absent"
                );
            }
            other => panic!("expected PeerRequest, got {other:?}"),
        }
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
    fn peer_response_accepted_with_handling_mode_rejected() {
        let req = CommsCommandRequest::PeerResponse {
            to: PeerName::new("peer-1").expect("peer"),
            in_reply_to: InteractionId(uuid::Uuid::now_v7()),
            status: ResponseStatus::Accepted,
            result: None,
            handling_mode: Some(HandlingMode::Steer),
        };
        let err = req
            .into_command(&crate::SessionId::new())
            .expect_err("accepted response with handling_mode must be rejected");
        assert_eq!(
            err,
            CommsCommandError::HandlingModeForbiddenForAcceptedResponse
        );
    }

    #[test]
    fn peer_response_completed_with_handling_mode_accepted() {
        let req = CommsCommandRequest::PeerResponse {
            to: PeerName::new("peer-1").expect("peer"),
            in_reply_to: InteractionId(uuid::Uuid::now_v7()),
            status: ResponseStatus::Completed,
            result: None,
            handling_mode: Some(HandlingMode::Steer),
        };
        assert!(
            req.into_command(&crate::SessionId::new()).is_ok(),
            "completed response with handling_mode=steer should be accepted"
        );
    }

    #[test]
    fn deserialize_input_with_typed_source() -> Result<(), serde_json::Error> {
        let json = r#"{"kind":"input","body":"hello","source":"webhook","handling_mode":"steer"}"#;
        let req: CommsCommandRequest = serde_json::from_str(json)?;
        match req {
            CommsCommandRequest::Input {
                body,
                source,
                handling_mode,
                ..
            } => {
                assert_eq!(body, "hello");
                assert_eq!(source, Some(InputSource::Webhook));
                assert_eq!(handling_mode, Some(HandlingMode::Steer));
            }
            other => panic!("expected Input, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn deserialize_input_invalid_source_rejects_at_serde_boundary() {
        let json = r#"{"kind":"input","body":"hello","source":"webhookd"}"#;
        let err = serde_json::from_str::<CommsCommandRequest>(json)
            .expect_err("invalid source must fail deserialization");
        let msg = err.to_string();
        // serde reports "unknown variant `webhookd`, expected one of ...".
        assert!(
            msg.contains("webhookd"),
            "error should name the rejected value, got: {msg}"
        );
    }

    #[test]
    fn deserialize_unknown_kind_rejects_at_serde_boundary() {
        let json = r#"{"kind":"foobar","body":"hello"}"#;
        let err = serde_json::from_str::<CommsCommandRequest>(json)
            .expect_err("unknown kind must fail deserialization");
        let msg = err.to_string();
        assert!(
            msg.contains("foobar") || msg.contains("variant"),
            "error should mention unknown variant, got: {msg}"
        );
    }
}
