//! Wire session types.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use meerkat_core::{
    AssistantBlock, BlobId, ContentBlock, ContentInput, ImageData, Message, ProviderMeta,
    SessionHistoryPage, SessionId, SessionInfo, SessionSummary, StopReason,
};

/// Canonical session info for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionInfo {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub is_active: bool,
    pub model: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl From<SessionInfo> for WireSessionInfo {
    fn from(info: SessionInfo) -> Self {
        Self {
            session_id: info.session_id,
            session_ref: None,
            created_at: info
                .created_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            updated_at: info
                .updated_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            message_count: info.message_count,
            is_active: info.is_active,
            model: info.model,
            provider: info.provider.as_str().to_string(),
            last_assistant_text: info.last_assistant_text,
            labels: info.labels,
        }
    }
}

/// Canonical session summary for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionSummary {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub total_tokens: u64,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl From<SessionSummary> for WireSessionSummary {
    fn from(summary: SessionSummary) -> Self {
        Self {
            session_id: summary.session_id,
            session_ref: None,
            created_at: summary
                .created_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            updated_at: summary
                .updated_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            message_count: summary.message_count,
            total_tokens: summary.total_tokens,
            is_active: summary.is_active,
            labels: summary.labels,
        }
    }
}

/// Provider continuity metadata for transcript blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum WireProviderMeta {
    Anthropic {
        signature: String,
    },
    AnthropicRedacted {
        data: String,
    },
    AnthropicCompaction {
        content: String,
    },
    Gemini {
        #[serde(rename = "thoughtSignature")]
        thought_signature: String,
    },
    OpenAi {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
    },
    Unknown,
}

impl From<ProviderMeta> for WireProviderMeta {
    fn from(value: ProviderMeta) -> Self {
        match value {
            ProviderMeta::Anthropic { signature } => Self::Anthropic { signature },
            ProviderMeta::AnthropicRedacted { data } => Self::AnthropicRedacted { data },
            ProviderMeta::AnthropicCompaction { content } => Self::AnthropicCompaction { content },
            ProviderMeta::Gemini { thought_signature } => Self::Gemini { thought_signature },
            ProviderMeta::OpenAi {
                id,
                encrypted_content,
            } => Self::OpenAi {
                id,
                encrypted_content,
            },
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum WireImageData {
    Inline {
        data: String,
    },
    Blob {
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        blob_id: BlobId,
    },
}

impl From<String> for WireImageData {
    fn from(data: String) -> Self {
        Self::Inline { data }
    }
}

impl From<&str> for WireImageData {
    fn from(data: &str) -> Self {
        Self::Inline {
            data: data.to_string(),
        }
    }
}

/// Wire-safe content block (no `source_path` — internal only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        #[serde(flatten)]
        data: WireImageData,
    },
    /// Forward-compatibility for unknown block types.
    #[serde(other)]
    Unknown,
}

impl From<ContentBlock> for WireContentBlock {
    fn from(block: ContentBlock) -> Self {
        match block {
            ContentBlock::Text { text } => WireContentBlock::Text { text },
            ContentBlock::Image { media_type, data } => WireContentBlock::Image {
                media_type,
                data: match data {
                    ImageData::Inline { data } => WireImageData::Inline { data },
                    ImageData::Blob { blob_id } => WireImageData::Blob { blob_id },
                },
            },
            _ => WireContentBlock::Unknown,
        }
    }
}

/// Wire-safe content input (mirrors `ContentInput`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum WireContentInput {
    Text(String),
    Blocks(Vec<WireContentBlock>),
}

impl From<ContentInput> for WireContentInput {
    fn from(input: ContentInput) -> Self {
        match input {
            ContentInput::Text(s) => WireContentInput::Text(s),
            ContentInput::Blocks(blocks) => {
                WireContentInput::Blocks(blocks.into_iter().map(Into::into).collect())
            }
        }
    }
}

/// Wire-safe tool result content that handles both legacy string and array formats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum WireToolResultContent {
    Text(String),
    Blocks(Vec<WireContentBlock>),
}

/// Transcript block inside a block-assistant message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "block_type", content = "data", rename_all = "snake_case")]
pub enum WireAssistantBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    Reasoning {
        #[serde(default)]
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    ToolUse {
        id: String,
        name: String,
        args: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    Unknown,
}

impl From<AssistantBlock> for WireAssistantBlock {
    fn from(value: AssistantBlock) -> Self {
        match value {
            AssistantBlock::Text { text, meta } => Self::Text {
                text,
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::Reasoning { text, meta } => Self::Reasoning {
                text,
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::ToolUse {
                id,
                name,
                args,
                meta,
            } => Self::ToolUse {
                id,
                name,
                args: serde_json::from_str(args.get()).unwrap_or(Value::Null),
                meta: meta.map(|m| (*m).into()),
            },
            _ => Self::Unknown,
        }
    }
}

/// Canonical stop reason for transcript messages.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireStopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    ContentFilter,
    Cancelled,
}

impl From<StopReason> for WireStopReason {
    fn from(value: StopReason) -> Self {
        match value {
            StopReason::EndTurn => Self::EndTurn,
            StopReason::ToolUse => Self::ToolUse,
            StopReason::MaxTokens => Self::MaxTokens,
            StopReason::StopSequence => Self::StopSequence,
            StopReason::ContentFilter => Self::ContentFilter,
            StopReason::Cancelled => Self::Cancelled,
        }
    }
}

/// Legacy assistant tool call payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// Tool result payload in a transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireToolResult {
    pub tool_use_id: String,
    pub content: WireToolResultContent,
    #[serde(default)]
    pub is_error: bool,
}

/// Canonical transcript message for public wire surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum WireSessionMessage {
    System {
        content: String,
    },
    User {
        content: WireContentInput,
    },
    Assistant {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireToolCall>,
        stop_reason: WireStopReason,
    },
    #[serde(rename = "block_assistant")]
    BlockAssistant {
        blocks: Vec<WireAssistantBlock>,
        stop_reason: WireStopReason,
    },
    #[serde(rename = "tool_results")]
    ToolResults {
        results: Vec<WireToolResult>,
    },
}

impl From<Message> for WireSessionMessage {
    fn from(value: Message) -> Self {
        match value {
            Message::System(message) => Self::System {
                content: message.content,
            },
            Message::User(message) => {
                let content = if message.content.len() == 1
                    && matches!(&message.content[0], ContentBlock::Text { .. })
                {
                    WireContentInput::Text(message.text_content())
                } else {
                    WireContentInput::Blocks(message.content.into_iter().map(Into::into).collect())
                };
                Self::User { content }
            }
            Message::Assistant(message) => Self::Assistant {
                content: message.content,
                tool_calls: message
                    .tool_calls
                    .into_iter()
                    .map(|tool_call| WireToolCall {
                        id: tool_call.id,
                        name: tool_call.name,
                        args: tool_call.args,
                    })
                    .collect(),
                stop_reason: message.stop_reason.into(),
            },
            Message::BlockAssistant(message) => Self::BlockAssistant {
                blocks: message.blocks.into_iter().map(Into::into).collect(),
                stop_reason: message.stop_reason.into(),
            },
            Message::ToolResults { results } => Self::ToolResults {
                results: results
                    .into_iter()
                    .map(|result| {
                        let content = if result.content.len() == 1
                            && matches!(&result.content[0], ContentBlock::Text { .. })
                        {
                            WireToolResultContent::Text(result.text_content())
                        } else {
                            WireToolResultContent::Blocks(
                                result.content.into_iter().map(Into::into).collect(),
                            )
                        };
                        WireToolResult {
                            tool_use_id: result.tool_use_id,
                            content,
                            is_error: result.is_error,
                        }
                    })
                    .collect(),
            },
        }
    }
}

/// Full session history in canonical wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionHistory {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub message_count: usize,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub has_more: bool,
    pub messages: Vec<WireSessionMessage>,
}

impl From<SessionHistoryPage> for WireSessionHistory {
    fn from(page: SessionHistoryPage) -> Self {
        Self {
            session_id: page.session_id,
            session_ref: None,
            message_count: page.message_count,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            messages: page.messages.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::time_compat::SystemTime;
    use meerkat_core::{
        AssistantMessage, BlockAssistantMessage, SystemMessage, ToolCall, UserMessage,
    };

    #[test]
    fn test_wire_session_summary_labels_roundtrip() {
        let mut labels = BTreeMap::new();
        labels.insert("env".to_string(), "prod".to_string());
        labels.insert("team".to_string(), "infra".to_string());

        let wire = WireSessionSummary {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 5,
            total_tokens: 100,
            is_active: true,
            labels: labels.clone(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, labels);
    }

    #[test]
    fn test_wire_session_summary_empty_labels_omitted() {
        let wire = WireSessionSummary {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 0,
            total_tokens: 0,
            is_active: false,
            labels: BTreeMap::new(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        assert!(
            !json.contains("\"labels\""),
            "empty labels should be omitted from JSON"
        );
    }

    #[test]
    fn test_wire_session_info_labels_roundtrip() {
        let mut labels = BTreeMap::new();
        labels.insert("role".to_string(), "orchestrator".to_string());

        let wire = WireSessionInfo {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 3,
            is_active: true,
            model: "claude-sonnet-4-5".to_string(),
            provider: "anthropic".to_string(),
            last_assistant_text: None,
            labels: labels.clone(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, labels);
    }

    #[test]
    fn test_wire_session_info_from_session_info_maps_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("env".to_string(), "staging".to_string());

        let info = SessionInfo {
            session_id: SessionId::new(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 2,
            is_active: false,
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            last_assistant_text: Some("hello".to_string()),
            labels: labels.clone(),
        };
        let wire: WireSessionInfo = info.into();
        assert_eq!(wire.labels, labels);
    }

    #[test]
    fn test_wire_session_summary_from_session_summary_maps_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("project".to_string(), "meerkat".to_string());

        let summary = SessionSummary {
            session_id: SessionId::new(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 10,
            total_tokens: 500,
            is_active: true,
            labels: labels.clone(),
        };
        let wire: WireSessionSummary = summary.into();
        assert_eq!(wire.labels, labels);
    }

    #[test]
    fn test_wire_session_info_backward_compat_no_labels() {
        let json = r#"{
            "session_id": "019405c8-1234-7000-8000-000000000001",
            "created_at": 1000,
            "updated_at": 2000,
            "message_count": 0,
            "is_active": false,
            "model": "claude-sonnet-4-5",
            "provider": "anthropic"
        }"#;
        let parsed: WireSessionInfo = serde_json::from_str(json).unwrap();
        assert!(parsed.labels.is_empty());
    }

    #[test]
    fn test_wire_session_summary_backward_compat_no_labels() {
        let json = r#"{
            "session_id": "019405c8-1234-7000-8000-000000000001",
            "created_at": 1000,
            "updated_at": 2000,
            "message_count": 0,
            "total_tokens": 0,
            "is_active": false
        }"#;
        let parsed: WireSessionSummary = serde_json::from_str(json).unwrap();
        assert!(parsed.labels.is_empty());
    }

    #[test]
    fn test_wire_session_history_roundtrip_mixed_messages() {
        let history = WireSessionHistory {
            session_id: SessionId::new(),
            session_ref: Some("session://example".to_string()),
            message_count: 5,
            offset: 0,
            limit: Some(5),
            has_more: false,
            messages: vec![
                WireSessionMessage::System {
                    content: "You are helpful".to_string(),
                },
                WireSessionMessage::User {
                    content: WireContentInput::Text("hello".to_string()),
                },
                WireSessionMessage::Assistant {
                    content: "hi".to_string(),
                    tool_calls: vec![WireToolCall {
                        id: "tool-1".to_string(),
                        name: "search".to_string(),
                        args: serde_json::json!({"q":"rust"}),
                    }],
                    stop_reason: WireStopReason::ToolUse,
                },
                WireSessionMessage::BlockAssistant {
                    blocks: vec![
                        WireAssistantBlock::Reasoning {
                            text: "thinking".to_string(),
                            meta: None,
                        },
                        WireAssistantBlock::Text {
                            text: "done".to_string(),
                            meta: None,
                        },
                    ],
                    stop_reason: WireStopReason::EndTurn,
                },
                WireSessionMessage::ToolResults {
                    results: vec![WireToolResult {
                        tool_use_id: "tool-1".to_string(),
                        content: WireToolResultContent::Text("ok".to_string()),
                        is_error: false,
                    }],
                },
            ],
        };
        let json = serde_json::to_string(&history).unwrap();
        let parsed: WireSessionHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, history);
    }

    #[test]
    fn test_wire_session_history_from_page_maps_messages() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 3,
            offset: 0,
            limit: None,
            has_more: false,
            messages: vec![
                Message::System(SystemMessage {
                    content: "sys".to_string(),
                }),
                Message::User(UserMessage::text("hi".to_string())),
                Message::Assistant(AssistantMessage {
                    content: "hello".to_string(),
                    tool_calls: vec![ToolCall::new(
                        "call-1".to_string(),
                        "search".to_string(),
                        serde_json::json!({"q":"meerkat"}),
                    )],
                    stop_reason: StopReason::ToolUse,
                    usage: meerkat_core::Usage::default(),
                }),
            ],
        };
        let wire: WireSessionHistory = page.into();
        assert_eq!(wire.messages.len(), 3);
        assert!(matches!(
            wire.messages[0],
            WireSessionMessage::System { .. }
        ));
        assert!(matches!(wire.messages[1], WireSessionMessage::User { .. }));
        assert!(matches!(
            wire.messages[2],
            WireSessionMessage::Assistant { .. }
        ));
    }

    #[test]
    fn test_wire_session_history_from_page_maps_block_assistant_and_tool_results() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 2,
            offset: 0,
            limit: Some(2),
            has_more: false,
            messages: vec![
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "hi".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "tool-2".to_string(),
                        "done".to_string(),
                        false,
                    )],
                },
            ],
        };
        let wire: WireSessionHistory = page.into();
        assert!(matches!(
            wire.messages[0],
            WireSessionMessage::BlockAssistant { .. }
        ));
        assert!(matches!(
            wire.messages[1],
            WireSessionMessage::ToolResults { .. }
        ));
    }

    #[test]
    fn test_wire_content_block_text_roundtrip() {
        let block = WireContentBlock::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: WireContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }

    #[test]
    fn test_wire_content_block_image_roundtrip() {
        let block = WireContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "iVBOR...".into(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: WireContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }

    #[test]
    fn test_wire_content_block_unknown_forward_compat() {
        let json = r#"{"type":"video","url":"https://example.com/v.mp4"}"#;
        let parsed: WireContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, WireContentBlock::Unknown);
    }

    #[test]
    fn test_wire_content_block_from_core_strips_source_path() {
        let core_block = ContentBlock::Image {
            media_type: "image/jpeg".to_string(),
            data: "base64data".into(),
        };
        let wire: WireContentBlock = core_block.into();
        assert_eq!(
            wire,
            WireContentBlock::Image {
                media_type: "image/jpeg".to_string(),
                data: "base64data".into(),
            }
        );
    }

    #[test]
    fn test_wire_content_input_text_roundtrip() {
        let input = WireContentInput::Text("hello world".to_string());
        let json = serde_json::to_string(&input).unwrap();
        assert_eq!(json, r#""hello world""#);
        let parsed: WireContentInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn test_wire_content_input_blocks_roundtrip() {
        let input = WireContentInput::Blocks(vec![
            WireContentBlock::Text {
                text: "look at this".to_string(),
            },
            WireContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc123".into(),
            },
        ]);
        let json = serde_json::to_string(&input).unwrap();
        let parsed: WireContentInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn test_wire_tool_result_content_text_roundtrip() {
        let content = WireToolResultContent::Text("result text".to_string());
        let json = serde_json::to_string(&content).unwrap();
        assert_eq!(json, r#""result text""#);
        let parsed: WireToolResultContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn test_wire_tool_result_content_blocks_roundtrip() {
        let content = WireToolResultContent::Blocks(vec![
            WireContentBlock::Text {
                text: "output".to_string(),
            },
            WireContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "data".into(),
            },
        ]);
        let json = serde_json::to_string(&content).unwrap();
        let parsed: WireToolResultContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn test_wire_tool_result_backward_compat_string() {
        let json = r#"{"tool_use_id":"t1","content":"hello","is_error":false}"#;
        let parsed: WireToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.content,
            WireToolResultContent::Text("hello".to_string())
        );
    }

    #[test]
    fn test_wire_user_message_text_backward_compat() {
        let json = r#"{"role":"user","content":"hello"}"#;
        let parsed: WireSessionMessage = serde_json::from_str(json).unwrap();
        match parsed {
            WireSessionMessage::User { content } => {
                assert_eq!(content, WireContentInput::Text("hello".to_string()));
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_wire_user_message_blocks() {
        let json = r#"{"role":"user","content":[{"type":"text","text":"look"},{"type":"image","media_type":"image/png","source":"inline","data":"abc"}]}"#;
        let parsed: WireSessionMessage = serde_json::from_str(json).unwrap();
        match parsed {
            WireSessionMessage::User { content } => {
                assert_eq!(
                    content,
                    WireContentInput::Blocks(vec![
                        WireContentBlock::Text {
                            text: "look".to_string()
                        },
                        WireContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "abc".into()
                        },
                    ])
                );
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_wire_user_message_from_multimodal_core() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 1,
            offset: 0,
            limit: None,
            has_more: false,
            messages: vec![Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "describe this".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "base64data".into(),
                },
            ]))],
        };
        let wire: WireSessionHistory = page.into();
        match &wire.messages[0] {
            WireSessionMessage::User { content } => {
                assert_eq!(
                    *content,
                    WireContentInput::Blocks(vec![
                        WireContentBlock::Text {
                            text: "describe this".to_string()
                        },
                        WireContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "base64data".into()
                        },
                    ])
                );
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_wire_tool_result_from_multimodal_core() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 1,
            offset: 0,
            limit: None,
            has_more: false,
            messages: vec![Message::ToolResults {
                results: vec![meerkat_core::ToolResult::with_blocks(
                    "tool-1".to_string(),
                    vec![
                        ContentBlock::Text {
                            text: "screenshot:".to_string(),
                        },
                        ContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "imgdata".into(),
                        },
                    ],
                    false,
                )],
            }],
        };
        let wire: WireSessionHistory = page.into();
        match &wire.messages[0] {
            WireSessionMessage::ToolResults { results } => {
                assert_eq!(
                    results[0].content,
                    WireToolResultContent::Blocks(vec![
                        WireContentBlock::Text {
                            text: "screenshot:".to_string()
                        },
                        WireContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "imgdata".into()
                        },
                    ])
                );
            }
            _ => panic!("expected ToolResults message"),
        }
    }
}
