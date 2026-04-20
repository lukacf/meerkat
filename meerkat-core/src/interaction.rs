//! Interaction types for the core agent loop.
//!
//! These types provide a simplified adapter layer in core (no comms dependency).
//! `CommsContent` in meerkat-comms remains canonical with richer types.
//! The comms runtime converts at the boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::comms::TrustedPeerSpec;
use crate::types::{ContentBlock, HandlingMode, RenderMetadata};

/// Unique identifier for an interaction.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InteractionId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl std::fmt::Display for InteractionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed status for response interactions.
///
/// Mirrors `CommsStatus` from `meerkat-comms` — the comms runtime converts at the boundary.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Accepted,
    Completed,
    Failed,
}

/// Canonical terminality classification of a `ResponseStatus`.
///
/// Consumers that care about progress-vs-terminal (and, within terminal,
/// whether the response completed or failed) must read this typed class
/// rather than re-`match` on `ResponseStatus` — that duplication is how
/// "terminal" drifts from one call site to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TerminalityClass {
    Progress,
    Terminal { disposition: TerminalDisposition },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TerminalDisposition {
    Completed,
    Failed,
}

/// Single source of truth for "is this response terminal?".
pub fn classify_response_terminality(status: ResponseStatus) -> TerminalityClass {
    match status {
        ResponseStatus::Accepted => TerminalityClass::Progress,
        ResponseStatus::Completed => TerminalityClass::Terminal {
            disposition: TerminalDisposition::Completed,
        },
        ResponseStatus::Failed => TerminalityClass::Terminal {
            disposition: TerminalDisposition::Failed,
        },
    }
}

/// Simplified interaction content for the core agent loop.
///
/// This is an adapter type — `CommsContent` in meerkat-comms has richer types
/// (`MessageIntent`, `CommsStatus`, etc.). The comms runtime converts at the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionContent {
    /// A simple text message.
    Message {
        body: String,
        /// Optional multimodal content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
    },
    /// A request for the agent to perform an action.
    Request { intent: String, params: Value },
    /// A response to a previous request.
    Response {
        in_reply_to: InteractionId,
        status: ResponseStatus,
        result: Value,
    },
}

/// An interaction drained from the inbox, ready for classification.
#[derive(Debug, Clone)]
pub struct InboxInteraction {
    /// Unique identifier for this interaction.
    pub id: InteractionId,
    /// Who sent this interaction (peer name or source label).
    pub from: String,
    /// The interaction content.
    pub content: InteractionContent,
    /// Pre-rendered text suitable for injection into an LLM session.
    pub rendered_text: String,
    /// Runtime-owned handling hint for ordinary work admitted from plain events.
    pub handling_mode: HandlingMode,
    /// Optional normalized rendering metadata carried alongside the interaction.
    pub render_metadata: Option<RenderMetadata>,
}

/// Canonical model-facing text projection for an external event.
///
/// The visible identity of an external event is its source label
/// (`webhook`, `rpc`, `stdin`, etc.). Optional body text may follow, but
/// structured payload remains typed metadata rather than prompt text.
pub fn format_external_event_projection(source_name: &str, body: Option<&str>) -> String {
    let label = format!("[EVENT via {source_name}]");
    let body = body.map(str::trim).filter(|body| !body.is_empty());

    match body {
        Some(body) => format!("{label} {body}"),
        None => label,
    }
}

/// Classification result for incoming peer/event traffic.
///
/// Stored with each inbox entry at ingress time. Downstream consumers
/// switch on this enum instead of re-classifying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerInputClass {
    /// A peer message that should route through canonical runtime admission.
    ActionableMessage,
    /// A peer request that should route through canonical runtime admission.
    ActionableRequest,
    /// A response to a previous outbound request (non-interrupting context).
    Response,
    /// Peer added lifecycle event.
    PeerLifecycleAdded,
    /// Peer retired lifecycle event.
    PeerLifecycleRetired,
    /// Peer unwired lifecycle event.
    PeerLifecycleUnwired,
    /// Member kickoff failed lifecycle event.
    PeerLifecycleKickoffFailed,
    /// Member kickoff cancelled lifecycle event.
    PeerLifecycleKickoffCancelled,
    /// A request whose intent is in the silent-intents set (inline-only, no LLM turn).
    SilentRequest,
    /// An ack envelope (filtered at ingress, never reaches agent loop).
    Ack,
    /// A plain (unauthenticated) event from an external source.
    PlainEvent,
}

impl PeerInputClass {
    /// Returns true if this class is actionable runtime ingress.
    pub fn is_actionable(&self) -> bool {
        matches!(
            self,
            Self::ActionableMessage
                | Self::ActionableRequest
                | Self::Response
                | Self::PlainEvent
                | Self::PeerLifecycleKickoffFailed
                | Self::PeerLifecycleKickoffCancelled
        )
    }
}

/// Canonical peer/event ingress candidate handed to runtime admission.
///
/// This is the typed, machine-authored drain unit for runtime-backed peer
/// ingress. It preserves ingress classification so downstream code does not
/// re-derive semantics after drain.
#[derive(Debug, Clone)]
pub struct PeerInputCandidate {
    /// The original interaction data.
    pub interaction: InboxInteraction,
    /// Pre-computed classification from ingress.
    pub class: PeerInputClass,
    /// For lifecycle events, the peer name that was added/retired.
    pub lifecycle_peer: Option<String>,
}

/// Back-compat alias for older runtime and diagnostic seams.
pub type ClassifiedInboxInteraction = PeerInputCandidate;

/// Coarse source kind for a queued peer-ingress item.
///
/// This is a diagnostic shape for MeerkatMachine mapping work. It records the
/// kind that was admitted at ingress without exposing transport internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerIngressKind {
    Message,
    Request,
    Response,
    Ack,
    PlainEvent,
}

/// Snapshot of one queued peer-ingress item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressEntrySnapshot {
    /// Stable ingress-time identity for this queued raw item.
    pub raw_item_id: String,
    /// Interaction/correlation identifier when one exists.
    pub interaction_id: Option<InteractionId>,
    /// Pre-computed ingress classification.
    pub class: PeerInputClass,
    /// Coarse admitted kind.
    pub kind: PeerIngressKind,
    /// Sender identity fixed at ingress time, if applicable.
    pub from_peer: Option<String>,
    /// Lifecycle peer identity for add/retire notices, if applicable.
    pub lifecycle_peer: Option<String>,
    /// Request envelope id or reply-to correlation when one exists.
    pub request_id: Option<String>,
    /// Whether this entry was trusted at ingress time, when peer authority
    /// owns the entry. Plain external events leave this unset.
    pub trusted_snapshot: Option<bool>,
}

/// Non-destructive snapshot of the queued peer-ingress surface.
///
/// This is intentionally queue-shaped rather than a full PeerComms model. It
/// is the current honest owner-visible slice of peer ingress while the broader
/// MeerkatMachine refactor proceeds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerIngressQueueSnapshot {
    pub total_count: usize,
    pub actionable_count: usize,
    pub response_count: usize,
    pub lifecycle_count: usize,
    pub silent_request_count: usize,
    pub ack_count: usize,
    pub plain_event_count: usize,
    pub queued_entries: Vec<PeerIngressEntrySnapshot>,
}

/// Canonical phase of the peer-ingress authority.
///
/// This is distinct from the raw classified queue snapshot: plain external
/// events can be queued while the peer authority itself remains `Absent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerIngressAuthorityPhase {
    #[default]
    Absent,
    Received,
    Dropped,
    Delivered,
}

/// Runtime-owned peer snapshot for the current Meerkat session.
///
/// This wraps the queued ingress surface with the trust membership that governs
/// which peer identities are admitted into that queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressRuntimeSnapshot {
    /// This runtime's public peer identity.
    pub self_peer_id: String,
    /// Whether unauthenticated peer envelopes are rejected at ingress.
    pub auth_required: bool,
    /// Current phase of the peer-ingress authority.
    pub authority_phase: PeerIngressAuthorityPhase,
    /// Current trusted peer set visible to this runtime.
    pub trusted_peers: Vec<TrustedPeerSpec>,
    /// Current length of the authority-owned typed peer submission queue.
    pub submission_queue_len: usize,
    /// Non-destructive snapshot of the queued ingress surface.
    pub queue: PeerIngressQueueSnapshot,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn interaction_id_json_roundtrip() {
        let id = InteractionId(Uuid::new_v4());
        let json = serde_json::to_string(&id).unwrap();
        let parsed: InteractionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn interaction_content_message_json_roundtrip() {
        let content = InteractionContent::Message {
            body: "hello".to_string(),
            blocks: None,
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "message");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn interaction_content_request_json_roundtrip() {
        let content = InteractionContent::Request {
            intent: "review".to_string(),
            params: serde_json::json!({"pr": 42}),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "request");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn interaction_content_response_json_roundtrip() {
        let id = InteractionId(Uuid::new_v4());
        let content = InteractionContent::Response {
            in_reply_to: id,
            status: ResponseStatus::Completed,
            result: serde_json::json!({"ok": true}),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "response");
        assert_eq!(json["status"], "completed");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn response_status_json_roundtrip_all_variants() {
        for (variant, expected_str) in [
            (ResponseStatus::Accepted, "accepted"),
            (ResponseStatus::Completed, "completed"),
            (ResponseStatus::Failed, "failed"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json, expected_str);
            let parsed: ResponseStatus = serde_json::from_value(json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn classify_response_terminality_covers_all_variants() {
        assert_eq!(
            classify_response_terminality(ResponseStatus::Accepted),
            TerminalityClass::Progress
        );
        assert_eq!(
            classify_response_terminality(ResponseStatus::Completed),
            TerminalityClass::Terminal {
                disposition: TerminalDisposition::Completed
            }
        );
        assert_eq!(
            classify_response_terminality(ResponseStatus::Failed),
            TerminalityClass::Terminal {
                disposition: TerminalDisposition::Failed
            }
        );
    }

    #[test]
    fn interaction_message_with_blocks_roundtrip() {
        let content = InteractionContent::Message {
            body: "hello".to_string(),
            blocks: Some(vec![
                ContentBlock::Text {
                    text: "hello".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".into(),
                },
            ]),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "message");
        assert!(json["blocks"].is_array());
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn inbox_interaction_preserves_runtime_hints() {
        let interaction = InboxInteraction {
            id: InteractionId(Uuid::new_v4()),
            from: "event:webhook".into(),
            content: InteractionContent::Message {
                body: "hello".into(),
                blocks: None,
            },
            rendered_text: "[EVENT via webhook] hello".into(),
            handling_mode: HandlingMode::Steer,
            render_metadata: Some(RenderMetadata {
                class: crate::types::RenderClass::SystemNotice,
                salience: crate::types::RenderSalience::Urgent,
            }),
        };

        assert_eq!(interaction.handling_mode, HandlingMode::Steer);
        assert!(interaction.render_metadata.is_some());
    }

    #[test]
    fn interaction_message_without_blocks_compat() {
        // Old format (no blocks field) should deserialize with blocks: None
        let old_json = r#"{"type":"message","body":"hello"}"#;
        let parsed: InteractionContent = serde_json::from_str(old_json).unwrap();
        match parsed {
            InteractionContent::Message { body, blocks } => {
                assert_eq!(body, "hello");
                assert_eq!(blocks, None);
            }
            other => panic!("Expected Message, got {other:?}"),
        }

        // Serialize with blocks: None should omit the field
        let content = InteractionContent::Message {
            body: "test".to_string(),
            blocks: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            value.get("blocks").is_none(),
            "blocks: None should not appear in JSON"
        );
    }
}
