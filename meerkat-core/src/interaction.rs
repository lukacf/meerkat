//! Interaction types for the core agent loop.
//!
//! These types provide a simplified adapter layer in core (no comms dependency).
//! `CommsContent` in meerkat-comms remains canonical with richer types.
//! The comms runtime converts at the boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Unique identifier for an interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InteractionId(pub Uuid);

/// Simplified interaction content for the core agent loop.
///
/// This is an adapter type — `CommsContent` in meerkat-comms has richer types
/// (`MessageIntent`, `CommsStatus`, etc.). The comms runtime converts at the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionContent {
    /// A simple text message.
    Message { body: String },
    /// A request for the agent to perform an action.
    Request { intent: String, params: Value },
    /// A response to a previous request.
    Response {
        in_reply_to: InteractionId,
        status: String,
        result: Value,
    },
}

/// An interaction drained from the inbox, ready for classification.
pub struct InboxInteraction {
    /// Unique identifier for this interaction.
    pub id: InteractionId,
    /// Who sent this interaction (peer name or source label).
    pub from: String,
    /// The interaction content.
    pub content: InteractionContent,
    /// Pre-rendered text suitable for injection into an LLM session.
    pub rendered_text: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
            status: "completed".to_string(),
            result: serde_json::json!({"ok": true}),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "response");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }
}
