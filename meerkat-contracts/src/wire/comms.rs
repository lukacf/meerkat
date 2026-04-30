//! Wire types for `comms/*`.
//!
//! These re-export the typed [`meerkat_core::comms::CommsCommandRequest`]
//! enum and its supporting discriminator enums so the schema-emit pipeline
//! can produce JSON schemas and SDK codegen can derive typed client bindings.
//!
//! The `comms/send` command payload remains flat:
//! `{ "session_id": "...", "kind": "<variant>", ... }`.

pub use meerkat_core::comms::{
    CommsCommandError, CommsCommandRequest, InputSource, InputStreamMode, PeerAddress,
    PeerCapabilitySet, PeerDirectoryEntry, PeerDirectoryListing, PeerDirectorySource, PeerId,
    PeerLifecycleKind, PeerName, PeerReachability, PeerReachabilityReason, PeerSendability,
    PeerTransport,
};
pub use meerkat_core::interaction::ResponseStatus;
pub use meerkat_core::types::HandlingMode;

use serde::{Deserialize, Serialize};

/// Request payload for `comms/send`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommsSendParams {
    Input {
        session_id: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<InputSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_self_session: Option<bool>,
    },
    PeerMessage {
        session_id: String,
        to: PeerId,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
    PeerLifecycle {
        session_id: String,
        to: PeerId,
        lifecycle_kind: PeerLifecycleKind,
        #[serde(default)]
        params: serde_json::Value,
    },
    PeerRequest {
        session_id: String,
        to: PeerId,
        intent: String,
        #[serde(default)]
        params: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
    },
    PeerResponse {
        session_id: String,
        to: PeerId,
        in_reply_to: meerkat_core::interaction::InteractionId,
        status: ResponseStatus,
        #[serde(default)]
        result: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
}

impl CommsSendParams {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Input { session_id, .. }
            | Self::PeerMessage { session_id, .. }
            | Self::PeerLifecycle { session_id, .. }
            | Self::PeerRequest { session_id, .. }
            | Self::PeerResponse { session_id, .. } => session_id,
        }
    }

    /// Recipient peer id for error normalization, if the command targets one.
    pub fn peer_label(&self) -> Option<String> {
        match self {
            Self::Input { .. } => None,
            Self::PeerMessage { to, .. }
            | Self::PeerLifecycle { to, .. }
            | Self::PeerRequest { to, .. }
            | Self::PeerResponse { to, .. } => Some(to.to_string()),
        }
    }

    pub fn into_command(self) -> CommsCommandRequest {
        match self {
            Self::Input {
                body,
                blocks,
                source,
                stream,
                handling_mode,
                allow_self_session,
                ..
            } => CommsCommandRequest::Input {
                body,
                blocks,
                source,
                stream,
                handling_mode,
                allow_self_session,
            },
            Self::PeerMessage {
                to,
                body,
                blocks,
                handling_mode,
                ..
            } => CommsCommandRequest::PeerMessage {
                to,
                body,
                blocks,
                handling_mode,
            },
            Self::PeerLifecycle {
                to,
                lifecycle_kind,
                params,
                ..
            } => CommsCommandRequest::PeerLifecycle {
                to,
                lifecycle_kind,
                params,
            },
            Self::PeerRequest {
                to,
                intent,
                params,
                handling_mode,
                stream,
                ..
            } => CommsCommandRequest::PeerRequest {
                to,
                intent,
                params,
                handling_mode,
                stream,
            },
            Self::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                handling_mode,
                ..
            } => CommsCommandRequest::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                handling_mode,
            },
        }
    }
}

/// Request payload for `comms/peers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CommsPeersParams {
    pub session_id: String,
}

/// Response payload for `comms/send`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommsSendResult {
    InputAccepted {
        interaction_id: String,
        stream_reserved: bool,
    },
    PeerMessageSent {
        envelope_id: String,
        acked: bool,
    },
    PeerLifecycleSent {
        envelope_id: String,
    },
    PeerRequestSent {
        envelope_id: String,
        interaction_id: String,
        request_id: String,
        stream_reserved: bool,
    },
    PeerResponseSent {
        envelope_id: String,
        in_reply_to: String,
    },
}

impl From<meerkat_core::comms::SendReceipt> for CommsSendResult {
    fn from(receipt: meerkat_core::comms::SendReceipt) -> Self {
        match receipt {
            meerkat_core::comms::SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => Self::InputAccepted {
                interaction_id: interaction_id.0.to_string(),
                stream_reserved,
            },
            meerkat_core::comms::SendReceipt::PeerMessageSent { envelope_id, acked } => {
                Self::PeerMessageSent {
                    envelope_id: envelope_id.to_string(),
                    acked,
                }
            }
            meerkat_core::comms::SendReceipt::PeerLifecycleSent { envelope_id } => {
                Self::PeerLifecycleSent {
                    envelope_id: envelope_id.to_string(),
                }
            }
            meerkat_core::comms::SendReceipt::PeerRequestSent {
                envelope_id,
                interaction_id,
                stream_reserved,
            } => {
                let envelope_id = envelope_id.to_string();
                Self::PeerRequestSent {
                    request_id: envelope_id.clone(),
                    envelope_id,
                    interaction_id: interaction_id.0.to_string(),
                    stream_reserved,
                }
            }
            meerkat_core::comms::SendReceipt::PeerResponseSent {
                envelope_id,
                in_reply_to,
            } => Self::PeerResponseSent {
                envelope_id: envelope_id.to_string(),
                in_reply_to: in_reply_to.0.to_string(),
            },
        }
    }
}

pub type CommsPeerEntry = PeerDirectoryEntry;

/// Response payload for `comms/peers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CommsPeersResult {
    pub peers: Vec<PeerDirectoryEntry>,
}

impl CommsPeersResult {
    pub fn from_entries(entries: &[meerkat_core::comms::PeerDirectoryEntry]) -> Self {
        Self {
            peers: entries.to_vec(),
        }
    }
}

impl From<PeerDirectoryListing> for CommsPeersResult {
    fn from(listing: PeerDirectoryListing) -> Self {
        Self {
            peers: listing.peers,
        }
    }
}
