use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InteractionId(pub Uuid);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionContent {
    Message {
        body: String,
    },
    Request {
        intent: String,
        params: Value,
    },
    Response {
        in_reply_to: InteractionId,
        status: String,
        result: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxInteraction {
    pub id: InteractionId,
    pub from: String,
    pub content: InteractionContent,
    pub rendered_text: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn interaction_id_json_roundtrip() {
        let original = InteractionId(Uuid::now_v7());
        let json = serde_json::to_value(original).unwrap();
        let parsed: InteractionId = serde_json::from_value(json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn interaction_content_json_roundtrip() {
        let cases = vec![
            InteractionContent::Message {
                body: "hello".to_string(),
            },
            InteractionContent::Request {
                intent: "review".to_string(),
                params: serde_json::json!({"a": 1}),
            },
            InteractionContent::Response {
                in_reply_to: InteractionId(Uuid::now_v7()),
                status: "completed".to_string(),
                result: serde_json::json!({"ok": true}),
            },
        ];
        for case in cases {
            let json = serde_json::to_value(case.clone()).unwrap();
            let parsed: InteractionContent = serde_json::from_value(json).unwrap();
            assert_eq!(case, parsed);
        }
    }

    #[test]
    fn inbox_interaction_json_roundtrip() {
        let original = InboxInteraction {
            id: InteractionId(Uuid::now_v7()),
            from: "peer-a".to_string(),
            content: InteractionContent::Message {
                body: "body".to_string(),
            },
            rendered_text: "rendered".to_string(),
        };
        let json = serde_json::to_value(original.clone()).unwrap();
        let parsed: InboxInteraction = serde_json::from_value(json).unwrap();
        assert_eq!(original, parsed);
    }
}
