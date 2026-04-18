use serde::{Deserialize, Serialize};

use super::{
    AudioConfig, Item, MaxTokens, Metadata, OutputModalities, Temperature, Tool, ToolChoice, Voice,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConversationMode {
    #[default]
    Auto,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResponseConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationMode>,
    /// Free-form metadata for the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<super::Modality>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_modalities: Option<OutputModalities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_format: Option<super::AudioFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<InputItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<Voice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Temperature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<MaxTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    ItemReference {
        id: String,
    },
    Message {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        role: super::Role,
        content: Vec<super::ContentPart>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    InProgress,
    Completed,
    Cancelled,
    Failed,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub object: String,
    pub conversation_id: Option<String>,
    pub status: ResponseStatus,
    pub status_details: Option<ResponseStatusDetails>,
    pub output: Option<Vec<Item>>,
    pub output_modalities: Option<OutputModalities>,
    pub max_output_tokens: Option<MaxTokens>,
    pub audio: Option<AudioConfig>,
    /// Free-form metadata for the response.
    pub metadata: Option<Metadata>,
    pub usage: Option<super::Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseStatusDetails {
    pub reason: Option<String>,
    pub error: Option<crate::error::ServerError>,
}
