//! Wire types for `comms/*`.
//!
//! The public wire owns its request/result contract and projects into the
//! core comms command envelope only after serde has closed over intent,
//! params, and result shape.
//!
//! The `comms/send` command payload remains flat:
//! `{ "session_id": "...", "kind": "<variant>", ... }`.

pub use meerkat_core::comms::{
    CommsCommandError, CommsPeerRequestIntent, InputSource, InputStreamMode, PeerAddress,
    PeerCapabilitySet, PeerDirectoryEntry, PeerDirectoryListing, PeerDirectorySource, PeerId,
    PeerLifecycleKind, PeerName, PeerSendability, PeerTransport, SendTaintOverride,
    SenderContentTaint,
};
pub use meerkat_core::interaction::ResponseStatus;
pub use meerkat_core::types::HandlingMode;

use super::supervisor_bridge::{BridgeCommand, BridgePeerSpec, BridgeReply};
use serde::{Deserialize, Serialize};

/// Typed params for one-way peer lifecycle notifications.
///
/// This is the public wire projection of the topology-update payloads that
/// used to travel as arbitrary JSON. `peer_spec` is the canonical typed
/// identity when the sender has it; `peer`, `role`, and `description` remain
/// inert presentation metadata for older projections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CommsPeerLifecycleParams {
    pub peer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_spec: Option<BridgePeerSpec>,
}

impl CommsPeerLifecycleParams {
    fn into_json_value(self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

/// Typed params for the actionable checksum-token request used by peer
/// request/response terminal-flow fixtures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CommsChecksumTokenParams {
    pub subject: String,
}

/// Closed discriminator carried in [`CommsChecksumTokenResult`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CommsChecksumTokenResultIntent {
    #[serde(rename = "checksum_token")]
    ChecksumToken,
}

/// Typed result for a checksum-token peer response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CommsChecksumTokenResult {
    pub request_intent: CommsChecksumTokenResultIntent,
    pub request_subject: String,
    pub token: String,
}

/// Typed params for public `peer_request`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum CommsPeerRequestParams {
    SupervisorBridge(Box<BridgeCommand>),
    ChecksumToken(CommsChecksumTokenParams),
}

impl CommsPeerRequestParams {
    fn matches_intent(&self, intent: &CommsPeerRequestIntent) -> bool {
        matches!(
            (intent, self),
            (
                CommsPeerRequestIntent::SupervisorBridge,
                Self::SupervisorBridge(_)
            ) | (
                CommsPeerRequestIntent::ChecksumToken,
                Self::ChecksumToken(_)
            )
        )
    }

    fn into_json_value(self) -> Result<serde_json::Value, serde_json::Error> {
        match self {
            Self::SupervisorBridge(params) => serde_json::to_value(params),
            Self::ChecksumToken(params) => serde_json::to_value(params),
        }
    }
}

/// Typed result payload for public `peer_response`.
///
/// Compatibility JSON is intentionally not accepted here. Callers that include
/// a `result` field must provide a typed bridge reply shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum CommsPeerResponseResult {
    SupervisorBridge(BridgeReply),
    ChecksumToken(CommsChecksumTokenResult),
}

impl CommsPeerResponseResult {
    fn into_json_value(self) -> Result<serde_json::Value, serde_json::Error> {
        match self {
            Self::SupervisorBridge(result) => serde_json::to_value(result),
            Self::ChecksumToken(result) => serde_json::to_value(result),
        }
    }
}

impl From<BridgeReply> for CommsPeerResponseResult {
    fn from(reply: BridgeReply) -> Self {
        Self::SupervisorBridge(reply)
    }
}

/// Failure projecting the typed public comms contract into the core
/// compatibility command envelope.
#[derive(Debug, thiserror::Error)]
pub enum CommsCommandProjectionError {
    #[error(transparent)]
    Command(#[from] CommsCommandError),
    #[error("peer_request params do not match typed intent {intent}")]
    IntentParamsMismatch { intent: &'static str },
    #[error("failed to project typed comms {field} to compatibility JSON: {source}")]
    CompatibilityJson {
        field: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl CommsCommandProjectionError {
    fn compatibility_json(field: &'static str, source: serde_json::Error) -> Self {
        Self::CompatibilityJson { field, source }
    }
}

/// Project the canonical `body` text from the single content authority.
///
/// When a comms command carries structured `blocks`, those blocks are the sole
/// content authority and `body` is a deterministic projection of their text
/// blocks (text-block contents joined with newlines; non-text blocks such as
/// images contribute no body text). This guarantees `body` cannot diverge from
/// the block content it mirrors. With no blocks, the caller-supplied `body`
/// stands alone as the sole content owner.
///
/// This mirrors the single-content-authority contract already enforced by the
/// comms tool surface (`project_body_from_blocks`); pulling the projection into
/// the wire boundary means every surface that deserializes the public comms
/// contract gets the same non-divergent guarantee for free.
fn single_authority_body(body: String, blocks: &Option<Vec<meerkat_core::ContentBlock>>) -> String {
    match blocks {
        Some(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                meerkat_core::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => body,
    }
}

/// Request command carried inside public `comms/send` surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommsCommandRequest {
    Input {
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
        to: PeerId,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        /// Per-send tri-state taint override: absent inherits the
        /// runtime-level outbound declaration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SendTaintOverride>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
    PeerLifecycle {
        to: PeerId,
        lifecycle_kind: PeerLifecycleKind,
        params: CommsPeerLifecycleParams,
    },
    PeerRequest {
        to: PeerId,
        intent: CommsPeerRequestIntent,
        params: CommsPeerRequestParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        /// Per-send tri-state taint override: absent inherits the
        /// runtime-level outbound declaration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SendTaintOverride>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<InputStreamMode>,
    },
    PeerResponse {
        to: PeerId,
        in_reply_to: meerkat_core::interaction::InteractionId,
        status: ResponseStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<CommsPeerResponseResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        /// Per-send tri-state taint override: absent inherits the
        /// runtime-level outbound declaration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SendTaintOverride>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
}

impl CommsCommandRequest {
    pub fn peer_label(&self) -> Option<String> {
        match self {
            Self::Input { .. } => None,
            Self::PeerMessage { to, .. }
            | Self::PeerLifecycle { to, .. }
            | Self::PeerRequest { to, .. }
            | Self::PeerResponse { to, .. } => Some(to.to_string()),
        }
    }

    pub fn into_core_request(
        self,
    ) -> Result<meerkat_core::comms::CommsCommandRequest, CommsCommandProjectionError> {
        Ok(match self {
            Self::Input {
                body,
                blocks,
                source,
                stream,
                handling_mode,
                allow_self_session,
            } => meerkat_core::comms::CommsCommandRequest::Input {
                body: single_authority_body(body, &blocks),
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
                content_taint,
                handling_mode,
            } => meerkat_core::comms::CommsCommandRequest::PeerMessage {
                to,
                body: single_authority_body(body, &blocks),
                blocks,
                content_taint,
                handling_mode,
            },
            Self::PeerLifecycle {
                to,
                lifecycle_kind,
                params,
            } => meerkat_core::comms::CommsCommandRequest::PeerLifecycle {
                to,
                lifecycle_kind,
                params: params.into_json_value().map_err(|source| {
                    CommsCommandProjectionError::compatibility_json("peer_lifecycle.params", source)
                })?,
            },
            Self::PeerRequest {
                to,
                intent,
                params,
                blocks,
                content_taint,
                handling_mode,
                stream,
            } => {
                if !params.matches_intent(&intent) {
                    return Err(CommsCommandProjectionError::IntentParamsMismatch {
                        intent: intent.as_str(),
                    });
                }
                meerkat_core::comms::CommsCommandRequest::PeerRequest {
                    to,
                    // The closed intent is now the same core-owned type on both
                    // sides of this seam, so it passes through typed — no
                    // string downgrade at the public-wire -> core projection.
                    // `params` cannot follow yet: its `supervisor.bridge`
                    // variant wraps the contracts-owned `BridgeCommand`, which
                    // core cannot reference, so the structurally-validated
                    // params still serialize to compatibility JSON here.
                    intent,
                    params: params.into_json_value().map_err(|source| {
                        CommsCommandProjectionError::compatibility_json(
                            "peer_request.params",
                            source,
                        )
                    })?,
                    blocks,
                    content_taint,
                    handling_mode,
                    stream,
                }
            }
            Self::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                blocks,
                content_taint,
                handling_mode,
            } => meerkat_core::comms::CommsCommandRequest::PeerResponse {
                to,
                in_reply_to,
                status,
                result: match result {
                    Some(result) => result.into_json_value().map_err(|source| {
                        CommsCommandProjectionError::compatibility_json(
                            "peer_response.result",
                            source,
                        )
                    })?,
                    None => serde_json::Value::Null,
                },
                blocks,
                content_taint,
                handling_mode,
            },
        })
    }

    pub fn into_command(
        self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::comms::CommsCommand, CommsCommandProjectionError> {
        Ok(self.into_core_request()?.into_command(session_id)?)
    }
}

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
        /// Per-send tri-state taint override: absent inherits the
        /// runtime-level outbound declaration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SendTaintOverride>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handling_mode: Option<HandlingMode>,
    },
    PeerLifecycle {
        session_id: String,
        to: PeerId,
        lifecycle_kind: PeerLifecycleKind,
        params: CommsPeerLifecycleParams,
    },
    PeerRequest {
        session_id: String,
        to: PeerId,
        intent: CommsPeerRequestIntent,
        params: CommsPeerRequestParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        /// Per-send tri-state taint override: absent inherits the
        /// runtime-level outbound declaration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SendTaintOverride>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<CommsPeerResponseResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<meerkat_core::ContentBlock>>,
        /// Per-send tri-state taint override: absent inherits the
        /// runtime-level outbound declaration.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_taint: Option<SendTaintOverride>,
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
                content_taint,
                handling_mode,
                ..
            } => CommsCommandRequest::PeerMessage {
                to,
                body,
                blocks,
                content_taint,
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
                blocks,
                content_taint,
                handling_mode,
                stream,
                ..
            } => CommsCommandRequest::PeerRequest {
                to,
                intent,
                params,
                blocks,
                content_taint,
                handling_mode,
                stream,
            },
            Self::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                blocks,
                content_taint,
                handling_mode,
                ..
            } => CommsCommandRequest::PeerResponse {
                to,
                in_reply_to,
                status,
                result,
                blocks,
                content_taint,
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
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommsSendResult {
    InputAccepted {
        interaction_id: String,
        stream_reserved: bool,
    },
    PeerMessageSent {
        envelope_id: String,
        delivery: meerkat_core::comms::PeerDeliveryOutcome,
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
            meerkat_core::comms::SendReceipt::PeerMessageSent {
                envelope_id,
                delivery,
            } => Self::PeerMessageSent {
                envelope_id: envelope_id.to_string(),
                delivery,
            },
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

/// Why a peer was unreachable for a `comms/send` dispatch.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CommsPeerUnreachableReason {
    OfflineOrNoAck,
    TransportError,
}

/// Typed error data for `comms/send` failures.
///
/// The error taxonomy is owned here (K17): surfaces serialize this contract
/// as JSON-RPC `error.data` instead of hand-shaping per-surface payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CommsSendErrorData {
    PeerNotFoundOrNotTrusted {
        peer: String,
        message: String,
    },
    PeerUnreachable {
        peer: String,
        reason: CommsPeerUnreachableReason,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    PeerAdmissionDropped {
        peer: String,
        reason: meerkat_core::comms::AdmissionDropReason,
        message: String,
    },
    SendFailed {
        message: String,
    },
    InvalidCommand {
        message: String,
    },
}

impl CommsSendErrorData {
    /// Human-readable message carried by every variant.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::PeerNotFoundOrNotTrusted { message, .. }
            | Self::PeerUnreachable { message, .. }
            | Self::PeerAdmissionDropped { message, .. }
            | Self::SendFailed { message }
            | Self::InvalidCommand { message } => message,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::super::supervisor_bridge::SUPERVISOR_BRIDGE_INTENT;
    use super::*;
    use serde_json::json;

    fn peer_id() -> PeerId {
        PeerId::new()
    }

    fn bridge_peer_spec() -> serde_json::Value {
        let pubkey = [7u8; 32];
        json!({
            "name": "supervisor",
            "peer_id": PeerId::from_ed25519_pubkey(&pubkey).to_string(),
            "address": "inproc://supervisor",
            "pubkey": pubkey,
        })
    }

    fn supervisor_bridge_params() -> serde_json::Value {
        json!({
            "command": "observe_member",
            "supervisor": bridge_peer_spec(),
            "epoch": 1,
            "protocol_version": 2,
        })
    }

    #[test]
    fn peer_request_unknown_intent_fails_closed() {
        let err = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_request",
            "to": peer_id().to_string(),
            "intent": "local.default",
            "params": {}
        }))
        .expect_err("unknown public comms intent must fail at serde boundary");

        let message = err.to_string();
        assert!(
            message.contains("local.default") || message.contains("variant"),
            "error should mention the rejected intent, got: {message}"
        );
    }

    #[test]
    fn peer_request_malformed_params_cannot_be_typed_success() {
        let err = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_request",
            "to": peer_id().to_string(),
            "intent": "supervisor.bridge",
            "params": "not-an-object"
        }))
        .expect_err("malformed supervisor bridge params must not deserialize");

        let message = err.to_string();
        assert!(
            message.contains("invalid type")
                || message.contains("params")
                || message.contains("did not match any variant"),
            "expected typed params error, got: {message}"
        );
    }

    #[test]
    fn peer_response_malformed_result_cannot_be_typed_success() {
        let err = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_response",
            "to": peer_id().to_string(),
            "in_reply_to": uuid::Uuid::new_v4().to_string(),
            "status": "completed",
            "result": {
                "result": "ack",
                "ok": "yes"
            }
        }))
        .expect_err("malformed typed result must not deserialize");

        let message = err.to_string();
        assert!(
            message.contains("ok")
                || message.contains("invalid type")
                || message.contains("did not match any variant"),
            "expected typed result error, got: {message}"
        );
    }

    #[test]
    fn comms_send_result_unknown_field_fails_closed() {
        let err = serde_json::from_value::<CommsSendResult>(json!({
            "kind": "peer_request_sent",
            "envelope_id": uuid::Uuid::new_v4().to_string(),
            "interaction_id": uuid::Uuid::new_v4().to_string(),
            "request_id": uuid::Uuid::new_v4().to_string(),
            "stream_reserved": true,
            "extra_behavior": true
        }))
        .expect_err("unknown result fields must fail at serde boundary");

        let message = err.to_string();
        assert!(
            message.contains("extra_behavior") || message.contains("unknown field"),
            "expected unknown result field error, got: {message}"
        );
    }

    #[test]
    fn comms_send_error_data_preserves_admission_drop_reason() {
        let value = serde_json::to_value(CommsSendErrorData::PeerAdmissionDropped {
            peer: "peer-a".to_string(),
            reason: meerkat_core::comms::AdmissionDropReason::InboxFull,
            message: "peer 'peer-a' rejected envelope at ingress: inbox_full".to_string(),
        })
        .expect("admission drop error data should serialize");

        assert_eq!(value["code"], "peer_admission_dropped");
        assert_eq!(value["peer"], "peer-a");
        assert_eq!(value["reason"], "inbox_full");

        let roundtrip: CommsSendErrorData =
            serde_json::from_value(value).expect("admission drop error data should deserialize");
        assert!(matches!(
            roundtrip,
            CommsSendErrorData::PeerAdmissionDropped {
                reason: meerkat_core::comms::AdmissionDropReason::InboxFull,
                ..
            }
        ));
    }

    #[test]
    fn public_peer_request_projects_typed_intent_and_params_to_core() {
        let params = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_request",
            "to": peer_id().to_string(),
            "intent": "supervisor.bridge",
            "params": supervisor_bridge_params(),
            "handling_mode": "queue",
            "stream": "reserve_interaction"
        }))
        .expect("typed supervisor bridge request should deserialize");

        let session_id = meerkat_core::types::SessionId::new();
        let command = params
            .into_command()
            .into_command(&session_id)
            .expect("typed comms request should project to core command");

        let meerkat_core::comms::CommsCommand::PeerRequest {
            intent,
            params,
            handling_mode,
            stream,
            ..
        } = command
        else {
            panic!("expected core peer request");
        };

        assert_eq!(intent, SUPERVISOR_BRIDGE_INTENT);
        assert_eq!(params["command"], "observe_member");
        assert_eq!(handling_mode, HandlingMode::Queue);
        assert_eq!(stream, InputStreamMode::ReserveInteraction);
    }

    #[test]
    fn public_checksum_token_request_projects_typed_intent_and_params_to_core() {
        let params = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_request",
            "to": peer_id().to_string(),
            "intent": "checksum_token",
            "params": {
                "subject": "alpha beta gamma"
            },
            "handling_mode": "steer",
            "stream": "reserve_interaction"
        }))
        .expect("typed checksum token request should deserialize");

        let session_id = meerkat_core::types::SessionId::new();
        let command = params
            .into_command()
            .into_command(&session_id)
            .expect("typed checksum token request should project to core command");

        let meerkat_core::comms::CommsCommand::PeerRequest {
            intent,
            params,
            handling_mode,
            stream,
            ..
        } = command
        else {
            panic!("expected core peer request");
        };

        assert_eq!(intent, "checksum_token");
        assert_eq!(params["subject"], "alpha beta gamma");
        assert_eq!(handling_mode, HandlingMode::Steer);
        assert_eq!(stream, InputStreamMode::ReserveInteraction);
    }

    #[test]
    fn public_peer_request_rejects_intent_params_mismatch_before_dispatch() {
        let params = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_request",
            "to": peer_id().to_string(),
            "intent": "checksum_token",
            "params": supervisor_bridge_params()
        }))
        .expect("mismatched typed params can deserialize but must not project");

        let session_id = meerkat_core::types::SessionId::new();
        let err = params
            .into_command()
            .into_command(&session_id)
            .expect_err("intent/params mismatch must not become a core command");

        assert!(
            err.to_string().contains("checksum_token"),
            "expected mismatch error to name intent, got: {err}"
        );
    }

    #[test]
    fn public_peer_response_result_projects_typed_bridge_reply_to_core() {
        let params = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_response",
            "to": peer_id().to_string(),
            "in_reply_to": uuid::Uuid::new_v4().to_string(),
            "status": "completed",
            "result": {
                "result": "ack",
                "ok": true
            }
        }))
        .expect("typed bridge reply should deserialize");

        let session_id = meerkat_core::types::SessionId::new();
        let command = params
            .into_command()
            .into_command(&session_id)
            .expect("typed comms response should project to core command");

        let meerkat_core::comms::CommsCommand::PeerResponse { result, .. } = command else {
            panic!("expected core peer response");
        };

        assert_eq!(result["result"], "ack");
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn public_peer_response_result_projects_typed_checksum_token_to_core() {
        let params = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_response",
            "to": peer_id().to_string(),
            "in_reply_to": uuid::Uuid::new_v4().to_string(),
            "status": "completed",
            "result": {
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            }
        }))
        .expect("typed checksum token reply should deserialize");

        let session_id = meerkat_core::types::SessionId::new();
        let command = params
            .into_command()
            .into_command(&session_id)
            .expect("typed checksum token reply should project to core command");

        let meerkat_core::comms::CommsCommand::PeerResponse { result, .. } = command else {
            panic!("expected core peer response");
        };

        assert_eq!(result["request_intent"], "checksum_token");
        assert_eq!(result["request_subject"], "alpha beta gamma");
        assert_eq!(result["token"], "birch seventeen");
    }

    #[test]
    fn input_body_is_projected_from_blocks_single_authority() {
        // A divergent caller-supplied body must NOT survive projection: when
        // blocks are present they are the single content authority and body is
        // derived from their text blocks.
        let request = CommsCommandRequest::Input {
            body: "diverged caller body".to_string(),
            blocks: Some(vec![
                meerkat_core::ContentBlock::Text {
                    text: "authoritative line one".to_string(),
                },
                meerkat_core::ContentBlock::Text {
                    text: "authoritative line two".to_string(),
                },
            ]),
            source: None,
            stream: None,
            handling_mode: None,
            allow_self_session: None,
        };

        let core = request
            .into_core_request()
            .expect("input request should project to core");

        let meerkat_core::comms::CommsCommandRequest::Input { body, .. } = core else {
            panic!("expected core input request");
        };
        assert_eq!(body, "authoritative line one\nauthoritative line two");
    }

    #[test]
    fn peer_message_body_stands_alone_when_no_blocks() {
        // With no blocks, the caller-supplied body is the sole content owner and
        // is preserved verbatim.
        let request = CommsCommandRequest::PeerMessage {
            to: peer_id(),
            body: "lone body".to_string(),
            blocks: None,
            content_taint: None,
            handling_mode: None,
        };

        let core = request
            .into_core_request()
            .expect("peer message should project to core");

        let meerkat_core::comms::CommsCommandRequest::PeerMessage { body, .. } = core else {
            panic!("expected core peer message");
        };
        assert_eq!(body, "lone body");
    }

    #[test]
    fn peer_message_content_taint_defaults_absent_and_threads_to_core() {
        // Absent on the wire => `None` (inherit the runtime declaration) — a
        // different fact from an explicit `undeclared` strip or a declared
        // taint state.
        let inherited = serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_message",
            "to": peer_id().to_string(),
            "body": "hello"
        }))
        .expect("peer message without content_taint should deserialize");
        let command = inherited
            .into_command()
            .into_command(&meerkat_core::types::SessionId::new())
            .expect("peer message should project to core command");
        let meerkat_core::comms::CommsCommand::PeerMessage { content_taint, .. } = command else {
            panic!("expected core peer message");
        };
        assert_eq!(content_taint, None);

        for (wire, expected) in [
            (
                json!({"declare": "tainted"}),
                SendTaintOverride::Declare(SenderContentTaint::Tainted),
            ),
            (
                json!({"declare": "clean"}),
                SendTaintOverride::Declare(SenderContentTaint::Clean),
            ),
            (json!("undeclared"), SendTaintOverride::Undeclared),
        ] {
            let params = serde_json::from_value::<CommsSendParams>(json!({
                "session_id": "sid_1",
                "kind": "peer_message",
                "to": peer_id().to_string(),
                "body": "hello",
                "content_taint": wire
            }))
            .expect("peer message with content_taint should deserialize");
            let command = params
                .into_command()
                .into_command(&meerkat_core::types::SessionId::new())
                .expect("peer message should project to core command");
            let meerkat_core::comms::CommsCommand::PeerMessage { content_taint, .. } = command
            else {
                panic!("expected core peer message");
            };
            assert_eq!(content_taint, Some(expected));
        }
    }

    #[test]
    fn public_peer_response_result_rejects_checksum_token_without_subject() {
        serde_json::from_value::<CommsSendParams>(json!({
            "session_id": "sid_1",
            "kind": "peer_response",
            "to": peer_id().to_string(),
            "in_reply_to": uuid::Uuid::new_v4().to_string(),
            "status": "completed",
            "result": {
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            }
        }))
        .expect_err("checksum token replies must carry the request subject discriminator");
    }
}
