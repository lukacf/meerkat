use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{ArbitraryJson, AudioFormat, ItemStatus, McpError, McpToolInfo, Role};

/// Manual (de)serialization preserves unknown variants as raw JSON while keeping
/// strong typing for known items.
#[derive(Debug, Clone)]
pub enum Item {
    Message {
        id: Option<String>,
        status: Option<ItemStatus>,
        role: Role,
        content: Vec<ContentPart>,
    },
    FunctionCall {
        id: Option<String>,
        status: Option<ItemStatus>,
        name: String,
        call_id: String,
        arguments: String,
    },
    FunctionCallOutput {
        id: Option<String>,
        call_id: String,
        output: String,
    },
    McpCall {
        id: Option<String>,
        status: Option<ItemStatus>,
        call_id: String,
        server_label: String,
        name: String,
        arguments: String,
        approval_request_id: Option<String>,
        output: Option<String>,
        error: Option<McpError>,
    },
    McpListTools {
        id: Option<String>,
        status: Option<ItemStatus>,
        server_label: String,
        tools: Option<Vec<McpToolInfo>>,
    },
    McpApprovalRequest {
        id: Option<String>,
        status: Option<ItemStatus>,
        server_label: String,
        name: String,
        arguments: String,
    },
    McpApprovalResponse {
        id: Option<String>,
        status: Option<ItemStatus>,
        approval_request_id: String,
        approve: bool,
        reason: Option<String>,
    },
    Unknown(ArbitraryJson),
}

impl std::fmt::Display for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Message { .. } => "message",
            Self::FunctionCall { .. } => "function_call",
            Self::FunctionCallOutput { .. } => "function_call_output",
            Self::McpCall { .. } => "mcp_call",
            Self::McpListTools { .. } => "mcp_list_tools",
            Self::McpApprovalRequest { .. } => "mcp_approval_request",
            Self::McpApprovalResponse { .. } => "mcp_approval_response",
            Self::Unknown(_) => "unknown",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ItemRepr {
    Message {
        id: Option<String>,
        status: Option<ItemStatus>,
        role: Role,
        content: Vec<ContentPart>,
    },
    FunctionCall {
        id: Option<String>,
        status: Option<ItemStatus>,
        name: String,
        call_id: String,
        arguments: String,
    },
    FunctionCallOutput {
        id: Option<String>,
        call_id: String,
        output: String,
    },
    McpCall {
        id: Option<String>,
        status: Option<ItemStatus>,
        call_id: String,
        server_label: String,
        name: String,
        arguments: String,
        approval_request_id: Option<String>,
        output: Option<String>,
        error: Option<McpError>,
    },
    McpListTools {
        id: Option<String>,
        status: Option<ItemStatus>,
        server_label: String,
        tools: Option<Vec<McpToolInfo>>,
    },
    McpApprovalRequest {
        id: Option<String>,
        status: Option<ItemStatus>,
        server_label: String,
        name: String,
        arguments: String,
    },
    McpApprovalResponse {
        id: Option<String>,
        status: Option<ItemStatus>,
        approval_request_id: String,
        approve: bool,
        reason: Option<String>,
    },
}

impl From<ItemRepr> for Item {
    fn from(repr: ItemRepr) -> Self {
        match repr {
            ItemRepr::Message {
                id,
                status,
                role,
                content,
            } => Self::Message {
                id,
                status,
                role,
                content,
            },
            ItemRepr::FunctionCall {
                id,
                status,
                name,
                call_id,
                arguments,
            } => Self::FunctionCall {
                id,
                status,
                name,
                call_id,
                arguments,
            },
            ItemRepr::FunctionCallOutput {
                id,
                call_id,
                output,
            } => Self::FunctionCallOutput {
                id,
                call_id,
                output,
            },
            ItemRepr::McpCall {
                id,
                status,
                call_id,
                server_label,
                name,
                arguments,
                approval_request_id,
                output,
                error,
            } => Self::McpCall {
                id,
                status,
                call_id,
                server_label,
                name,
                arguments,
                approval_request_id,
                output,
                error,
            },
            ItemRepr::McpListTools {
                id,
                status,
                server_label,
                tools,
            } => Self::McpListTools {
                id,
                status,
                server_label,
                tools,
            },
            ItemRepr::McpApprovalRequest {
                id,
                status,
                server_label,
                name,
                arguments,
            } => Self::McpApprovalRequest {
                id,
                status,
                server_label,
                name,
                arguments,
            },
            ItemRepr::McpApprovalResponse {
                id,
                status,
                approval_request_id,
                approve,
                reason,
            } => Self::McpApprovalResponse {
                id,
                status,
                approval_request_id,
                approve,
                reason,
            },
        }
    }
}

impl Serialize for Item {
    #[allow(clippy::too_many_lines)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unknown(value) => value.serialize(serializer),
            Self::Message {
                id,
                status,
                role,
                content,
            } => {
                let mut state = serializer.serialize_struct("Item", 5)?;
                state.serialize_field("type", "message")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                if let Some(value) = status {
                    state.serialize_field("status", value)?;
                }
                state.serialize_field("role", role)?;
                state.serialize_field("content", content)?;
                state.end()
            }
            Self::FunctionCall {
                id,
                status,
                name,
                call_id,
                arguments,
            } => {
                let mut state = serializer.serialize_struct("Item", 6)?;
                state.serialize_field("type", "function_call")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                if let Some(value) = status {
                    state.serialize_field("status", value)?;
                }
                state.serialize_field("name", name)?;
                state.serialize_field("call_id", call_id)?;
                state.serialize_field("arguments", arguments)?;
                state.end()
            }
            Self::FunctionCallOutput {
                id,
                call_id,
                output,
            } => {
                let mut state = serializer.serialize_struct("Item", 4)?;
                state.serialize_field("type", "function_call_output")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                state.serialize_field("call_id", call_id)?;
                state.serialize_field("output", output)?;
                state.end()
            }
            Self::McpCall {
                id,
                status,
                call_id,
                server_label,
                name,
                arguments,
                approval_request_id,
                output,
                error,
            } => {
                let mut state = serializer.serialize_struct("Item", 9)?;
                state.serialize_field("type", "mcp_call")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                if let Some(value) = status {
                    state.serialize_field("status", value)?;
                }
                state.serialize_field("call_id", call_id)?;
                state.serialize_field("server_label", server_label)?;
                state.serialize_field("name", name)?;
                state.serialize_field("arguments", arguments)?;
                if let Some(value) = approval_request_id {
                    state.serialize_field("approval_request_id", value)?;
                }
                if let Some(value) = output {
                    state.serialize_field("output", value)?;
                }
                if let Some(value) = error {
                    state.serialize_field("error", value)?;
                }
                state.end()
            }
            Self::McpListTools {
                id,
                status,
                server_label,
                tools,
            } => {
                let mut state = serializer.serialize_struct("Item", 5)?;
                state.serialize_field("type", "mcp_list_tools")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                if let Some(value) = status {
                    state.serialize_field("status", value)?;
                }
                state.serialize_field("server_label", server_label)?;
                if let Some(value) = tools {
                    state.serialize_field("tools", value)?;
                }
                state.end()
            }
            Self::McpApprovalRequest {
                id,
                status,
                server_label,
                name,
                arguments,
            } => {
                let mut state = serializer.serialize_struct("Item", 6)?;
                state.serialize_field("type", "mcp_approval_request")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                if let Some(value) = status {
                    state.serialize_field("status", value)?;
                }
                state.serialize_field("server_label", server_label)?;
                state.serialize_field("name", name)?;
                state.serialize_field("arguments", arguments)?;
                state.end()
            }
            Self::McpApprovalResponse {
                id,
                status,
                approval_request_id,
                approve,
                reason,
            } => {
                let mut state = serializer.serialize_struct("Item", 6)?;
                state.serialize_field("type", "mcp_approval_response")?;
                if let Some(value) = id {
                    state.serialize_field("id", value)?;
                }
                if let Some(value) = status {
                    state.serialize_field("status", value)?;
                }
                state.serialize_field("approval_request_id", approval_request_id)?;
                state.serialize_field("approve", approve)?;
                if let Some(value) = reason {
                    state.serialize_field("reason", value)?;
                }
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = ArbitraryJson::deserialize(deserializer)?;
        match ItemRepr::deserialize(value.clone()) {
            Ok(repr) => Ok(repr.into()),
            Err(err) => {
                tracing::debug!("Failed to parse Item: {err}");
                Ok(Self::Unknown(value))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AudioPartFormat {
    Label(String),
    Config(AudioFormat),
}

/// Manual (de)serialization preserves unknown variants as raw JSON while keeping
/// strong typing for known parts.
#[derive(Debug, Clone)]
pub enum ContentPart {
    InputText {
        text: String,
    },
    InputAudio {
        audio: String,
        transcript: Option<String>,
        format: Option<AudioFormat>,
    },
    InputImage {
        image_url: String,
        detail: Option<String>,
    },
    OutputText {
        text: String,
    },
    OutputAudio {
        audio: Option<String>,
        transcript: Option<String>,
        format: Option<AudioFormat>,
    },
    Text {
        text: String,
    },
    Audio {
        audio: Option<String>,
        transcript: Option<String>,
        format: Option<AudioPartFormat>,
    },
    Unknown(ArbitraryJson),
}

impl std::fmt::Display for ContentPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::InputText { .. } => "input_text",
            Self::InputAudio { .. } => "input_audio",
            Self::InputImage { .. } => "input_image",
            Self::OutputText { .. } => "output_text",
            Self::OutputAudio { .. } => "output_audio",
            Self::Text { .. } => "text",
            Self::Audio { .. } => "audio",
            Self::Unknown(_) => "unknown",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPartRepr {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "input_audio")]
    InputAudio {
        audio: String,
        transcript: Option<String>,
        format: Option<AudioFormat>,
    },
    #[serde(rename = "input_image")]
    InputImage {
        image_url: String,
        detail: Option<String>,
    },
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "output_audio")]
    OutputAudio {
        #[serde(skip_serializing_if = "Option::is_none")]
        audio: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        transcript: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<AudioFormat>,
    },
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "audio")]
    Audio {
        audio: Option<String>,
        transcript: Option<String>,
        format: Option<AudioPartFormat>,
    },
}

impl From<ContentPartRepr> for ContentPart {
    fn from(repr: ContentPartRepr) -> Self {
        match repr {
            ContentPartRepr::InputText { text } => Self::InputText { text },
            ContentPartRepr::InputAudio {
                audio,
                transcript,
                format,
            } => Self::InputAudio {
                audio,
                transcript,
                format,
            },
            ContentPartRepr::InputImage { image_url, detail } => {
                Self::InputImage { image_url, detail }
            }
            ContentPartRepr::OutputText { text } => Self::OutputText { text },
            ContentPartRepr::OutputAudio {
                audio,
                transcript,
                format,
            } => Self::OutputAudio {
                audio,
                transcript,
                format,
            },
            ContentPartRepr::Text { text } => Self::Text { text },
            ContentPartRepr::Audio {
                audio,
                transcript,
                format,
            } => Self::Audio {
                audio,
                transcript,
                format,
            },
        }
    }
}

impl Serialize for ContentPart {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Unknown(value) => value.serialize(serializer),
            Self::InputText { text } => {
                let mut state = serializer.serialize_struct("ContentPart", 2)?;
                state.serialize_field("type", "input_text")?;
                state.serialize_field("text", text)?;
                state.end()
            }
            Self::InputAudio {
                audio,
                transcript,
                format,
            } => {
                let mut state = serializer.serialize_struct("ContentPart", 4)?;
                state.serialize_field("type", "input_audio")?;
                state.serialize_field("audio", audio)?;
                if let Some(value) = transcript {
                    state.serialize_field("transcript", value)?;
                }
                if let Some(value) = format {
                    state.serialize_field("format", value)?;
                }
                state.end()
            }
            Self::InputImage { image_url, detail } => {
                let mut state = serializer.serialize_struct("ContentPart", 3)?;
                state.serialize_field("type", "input_image")?;
                state.serialize_field("image_url", image_url)?;
                if let Some(value) = detail {
                    state.serialize_field("detail", value)?;
                }
                state.end()
            }
            Self::OutputText { text } => {
                let mut state = serializer.serialize_struct("ContentPart", 2)?;
                state.serialize_field("type", "output_text")?;
                state.serialize_field("text", text)?;
                state.end()
            }
            Self::OutputAudio {
                audio,
                transcript,
                format,
            } => {
                let mut state = serializer.serialize_struct("ContentPart", 4)?;
                state.serialize_field("type", "output_audio")?;
                if let Some(value) = audio {
                    state.serialize_field("audio", value)?;
                }
                if let Some(value) = transcript {
                    state.serialize_field("transcript", value)?;
                }
                if let Some(value) = format {
                    state.serialize_field("format", value)?;
                }
                state.end()
            }
            Self::Text { text } => {
                let mut state = serializer.serialize_struct("ContentPart", 2)?;
                state.serialize_field("type", "text")?;
                state.serialize_field("text", text)?;
                state.end()
            }
            Self::Audio {
                audio,
                transcript,
                format,
            } => {
                let mut state = serializer.serialize_struct("ContentPart", 4)?;
                state.serialize_field("type", "audio")?;
                if let Some(value) = audio {
                    state.serialize_field("audio", value)?;
                }
                if let Some(value) = transcript {
                    state.serialize_field("transcript", value)?;
                }
                if let Some(value) = format {
                    state.serialize_field("format", value)?;
                }
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ContentPart {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = ArbitraryJson::deserialize(deserializer)?;
        match ContentPartRepr::deserialize(value.clone()) {
            Ok(repr) => Ok(repr.into()),
            Err(err) => {
                tracing::debug!("Failed to parse ContentPart: {err}");
                Ok(Self::Unknown(value))
            }
        }
    }
}
