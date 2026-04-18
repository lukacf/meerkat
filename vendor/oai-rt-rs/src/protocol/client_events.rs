use super::models::{Item, ResponseConfig, SessionUpdate};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientEvent {
    #[serde(rename = "session.update")]
    SessionUpdate {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        session: Box<SessionUpdate>,
    },
    #[serde(rename = "input_audio_buffer.append")]
    InputAudioBufferAppend {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        audio: String,
    },
    #[serde(rename = "input_audio_buffer.commit")]
    InputAudioBufferCommit {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
    },
    #[serde(rename = "input_audio_buffer.clear")]
    InputAudioBufferClear {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
    },
    #[serde(rename = "conversation.item.create")]
    ConversationItemCreate {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        previous_item_id: Option<String>,
        item: Box<Item>,
    },
    #[serde(rename = "conversation.item.retrieve")]
    ConversationItemRetrieve {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        item_id: String,
    },
    #[serde(rename = "conversation.item.truncate")]
    ConversationItemTruncate {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        item_id: String,
        content_index: u32,
        audio_end_ms: u32,
    },
    #[serde(rename = "conversation.item.delete")]
    ConversationItemDelete {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        item_id: String,
    },
    #[serde(rename = "response.create")]
    ResponseCreate {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<Box<ResponseConfig>>,
    },
    #[serde(rename = "response.cancel")]
    ResponseCancel {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
    },
    #[serde(rename = "output_audio_buffer.clear")]
    OutputAudioBufferClear {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
    },
}
