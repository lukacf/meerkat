use super::models::{ArbitraryJson, ContentPart, Item, Response, Session, Usage};
use crate::error::ServerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum ServerEvent {
    Error {
        event_id: String,
        error: ServerError,
    },
    SessionCreated {
        event_id: String,
        session: Session,
    },
    SessionUpdated {
        event_id: String,
        session: Session,
    },
    ConversationItemCreated {
        event_id: String,
        previous_item_id: Option<String>,
        item: Item,
    },
    ConversationItemAdded {
        event_id: String,
        previous_item_id: Option<String>,
        item: Item,
    },
    ConversationItemDone {
        event_id: String,
        previous_item_id: Option<String>,
        item: Item,
    },
    ConversationItemRetrieved {
        event_id: String,
        item: Item,
    },
    ConversationItemDeleted {
        event_id: String,
        item_id: String,
    },
    ConversationItemTruncated {
        event_id: String,
        item_id: String,
        content_index: u32,
        audio_end_ms: u32,
    },
    InputAudioBufferCommitted {
        event_id: String,
        previous_item_id: Option<String>,
        item_id: String,
    },
    InputAudioBufferCleared {
        event_id: String,
    },
    InputAudioBufferSpeechStarted {
        event_id: String,
        audio_start_ms: u32,
        item_id: String,
    },
    InputAudioBufferSpeechStopped {
        event_id: String,
        audio_end_ms: u32,
        item_id: String,
    },
    InputAudioBufferTimeoutTriggered {
        event_id: String,
        item_id: String,
        audio_start_ms: u32,
        audio_end_ms: u32,
    },
    DtmfEventReceived {
        event: String,
        received_at: u64,
    },
    OutputAudioBufferStarted {
        event_id: String,
        response_id: String,
    },
    OutputAudioBufferStopped {
        event_id: String,
        response_id: String,
    },
    OutputAudioBufferCleared {
        event_id: String,
        response_id: String,
    },
    InputAudioTranscriptionDelta {
        event_id: String,
        item_id: String,
        content_index: u32,
        delta: String,
        obfuscation: Option<Value>,
        logprobs: Option<Value>,
    },
    InputAudioTranscriptionSegment {
        event_id: String,
        item_id: String,
        content_index: u32,
        text: String,
        id: Option<String>,
        speaker: Option<String>,
        start: Option<f64>,
        end: Option<f64>,
    },
    InputAudioTranscriptionFailed {
        event_id: String,
        item_id: String,
        content_index: u32,
        error: ServerError,
    },
    InputAudioTranscriptionCompleted {
        event_id: String,
        item_id: String,
        content_index: u32,
        transcript: String,
        logprobs: Option<Value>,
        usage: Option<Usage>,
    },
    McpListToolsInProgress {
        event_id: String,
        item_id: String,
    },
    McpListToolsCompleted {
        event_id: String,
        item_id: String,
    },
    McpListToolsFailed {
        event_id: String,
        item_id: String,
        error: Option<ServerError>,
    },
    ResponseCreated {
        event_id: String,
        response: Response,
    },
    ResponseDone {
        event_id: String,
        response: Response,
    },
    ResponseCancelled {
        event_id: String,
        response: Response,
    },
    ResponseOutputItemAdded {
        event_id: String,
        response_id: String,
        output_index: u32,
        item: Item,
    },
    ResponseOutputItemDone {
        event_id: String,
        response_id: String,
        output_index: u32,
        item: Item,
    },
    ResponseContentPartAdded {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
    },
    ResponseContentPartDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
    },
    ResponseOutputTextDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    ResponseOutputTextDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        text: String,
    },
    ResponseOutputAudioDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    ResponseOutputAudioDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        item: Option<Item>,
    },
    ResponseOutputAudioTranscriptDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    ResponseOutputAudioTranscriptDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        transcript: String,
    },
    ResponseFunctionCallArgumentsDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        call_id: String,
        delta: String,
    },
    ResponseFunctionCallArgumentsDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        call_id: String,
        name: String,
        arguments: String,
    },
    ResponseMcpCallArgumentsDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        delta: String,
        obfuscation: Option<Value>,
    },
    ResponseMcpCallArgumentsDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        arguments: String,
    },
    ResponseMcpCallInProgress {
        event_id: String,
        item_id: String,
        output_index: u32,
    },
    ResponseMcpCallCompleted {
        event_id: String,
        item_id: String,
        output_index: u32,
    },
    ResponseMcpCallFailed {
        event_id: String,
        item_id: String,
        output_index: u32,
    },
    RateLimitsUpdated {
        event_id: String,
        rate_limits: Vec<RateLimit>,
    },
    Unknown(ArbitraryJson),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
enum ServerEventRepr {
    #[serde(rename = "error")]
    Error {
        event_id: String,
        error: ServerError,
    },
    #[serde(rename = "session.created")]
    SessionCreated { event_id: String, session: Session },
    #[serde(rename = "session.updated")]
    SessionUpdated { event_id: String, session: Session },
    #[serde(rename = "conversation.item.created")]
    ConversationItemCreated {
        event_id: String,
        previous_item_id: Option<String>,
        item: Item,
    },
    #[serde(rename = "conversation.item.added")]
    ConversationItemAdded {
        event_id: String,
        previous_item_id: Option<String>,
        item: Item,
    },
    #[serde(rename = "conversation.item.done")]
    ConversationItemDone {
        event_id: String,
        previous_item_id: Option<String>,
        item: Item,
    },
    #[serde(rename = "conversation.item.retrieved")]
    ConversationItemRetrieved { event_id: String, item: Item },
    #[serde(rename = "conversation.item.deleted")]
    ConversationItemDeleted { event_id: String, item_id: String },
    #[serde(rename = "conversation.item.truncated")]
    ConversationItemTruncated {
        event_id: String,
        item_id: String,
        content_index: u32,
        audio_end_ms: u32,
    },
    #[serde(rename = "input_audio_buffer.committed")]
    InputAudioBufferCommitted {
        event_id: String,
        previous_item_id: Option<String>,
        item_id: String,
    },
    #[serde(rename = "input_audio_buffer.cleared")]
    InputAudioBufferCleared { event_id: String },
    #[serde(rename = "input_audio_buffer.speech_started")]
    InputAudioBufferSpeechStarted {
        event_id: String,
        audio_start_ms: u32,
        item_id: String,
    },
    #[serde(rename = "input_audio_buffer.speech_stopped")]
    InputAudioBufferSpeechStopped {
        event_id: String,
        audio_end_ms: u32,
        item_id: String,
    },
    #[serde(rename = "input_audio_buffer.timeout_triggered")]
    InputAudioBufferTimeoutTriggered {
        event_id: String,
        item_id: String,
        audio_start_ms: u32,
        audio_end_ms: u32,
    },
    #[serde(rename = "input_audio_buffer.dtmf_event_received")]
    DtmfEventReceived { event: String, received_at: u64 },
    #[serde(rename = "output_audio_buffer.started")]
    OutputAudioBufferStarted {
        event_id: String,
        response_id: String,
    },
    #[serde(rename = "output_audio_buffer.stopped")]
    OutputAudioBufferStopped {
        event_id: String,
        response_id: String,
    },
    #[serde(rename = "output_audio_buffer.cleared")]
    OutputAudioBufferCleared {
        event_id: String,
        response_id: String,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.delta")]
    InputAudioTranscriptionDelta {
        event_id: String,
        item_id: String,
        content_index: u32,
        delta: String,
        obfuscation: Option<Value>,
        logprobs: Option<Value>,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.segment")]
    InputAudioTranscriptionSegment {
        event_id: String,
        item_id: String,
        content_index: u32,
        text: String,
        id: Option<String>,
        speaker: Option<String>,
        start: Option<f64>,
        end: Option<f64>,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.failed")]
    InputAudioTranscriptionFailed {
        event_id: String,
        item_id: String,
        content_index: u32,
        error: ServerError,
    },
    #[serde(rename = "conversation.item.input_audio_transcription.completed")]
    InputAudioTranscriptionCompleted {
        event_id: String,
        item_id: String,
        content_index: u32,
        transcript: String,
        logprobs: Option<Value>,
        usage: Option<Usage>,
    },
    #[serde(rename = "mcp_list_tools.in_progress")]
    McpListToolsInProgress { event_id: String, item_id: String },
    #[serde(rename = "mcp_list_tools.completed")]
    McpListToolsCompleted { event_id: String, item_id: String },
    #[serde(rename = "mcp_list_tools.failed")]
    McpListToolsFailed {
        event_id: String,
        item_id: String,
        error: Option<ServerError>,
    },
    #[serde(rename = "response.created")]
    ResponseCreated {
        event_id: String,
        response: Response,
    },
    #[serde(rename = "response.done")]
    ResponseDone {
        event_id: String,
        response: Response,
    },
    #[serde(rename = "response.cancelled")]
    ResponseCancelled {
        event_id: String,
        response: Response,
    },
    #[serde(rename = "response.output_item.added")]
    ResponseOutputItemAdded {
        event_id: String,
        response_id: String,
        output_index: u32,
        item: Item,
    },
    #[serde(rename = "response.output_item.done")]
    ResponseOutputItemDone {
        event_id: String,
        response_id: String,
        output_index: u32,
        item: Item,
    },
    #[serde(rename = "response.content_part.added")]
    ResponseContentPartAdded {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
    },
    #[serde(rename = "response.content_part.done")]
    ResponseContentPartDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
    },
    #[serde(rename = "response.output_text.delta")]
    ResponseOutputTextDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    #[serde(rename = "response.output_text.done")]
    ResponseOutputTextDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        text: String,
    },
    #[serde(rename = "response.output_audio.delta")]
    ResponseOutputAudioDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    #[serde(rename = "response.output_audio.done")]
    ResponseOutputAudioDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        item: Option<Item>,
    },
    #[serde(rename = "response.output_audio_transcript.delta")]
    ResponseOutputAudioTranscriptDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    #[serde(rename = "response.output_audio_transcript.done")]
    ResponseOutputAudioTranscriptDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        transcript: String,
    },
    #[serde(rename = "response.function_call_arguments.delta")]
    ResponseFunctionCallArgumentsDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        call_id: String,
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    ResponseFunctionCallArgumentsDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "response.mcp_call_arguments.delta")]
    ResponseMcpCallArgumentsDelta {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        delta: String,
        obfuscation: Option<Value>,
    },
    #[serde(rename = "response.mcp_call_arguments.done")]
    ResponseMcpCallArgumentsDone {
        event_id: String,
        response_id: String,
        item_id: String,
        output_index: u32,
        arguments: String,
    },
    #[serde(rename = "response.mcp_call.in_progress")]
    ResponseMcpCallInProgress {
        event_id: String,
        item_id: String,
        output_index: u32,
    },
    #[serde(rename = "response.mcp_call.completed")]
    ResponseMcpCallCompleted {
        event_id: String,
        item_id: String,
        output_index: u32,
    },
    #[serde(rename = "response.mcp_call.failed")]
    ResponseMcpCallFailed {
        event_id: String,
        item_id: String,
        output_index: u32,
    },
    #[serde(rename = "rate_limits.updated")]
    RateLimitsUpdated {
        event_id: String,
        rate_limits: Vec<RateLimit>,
    },
}

impl From<ServerEventRepr> for ServerEvent {
    #[allow(clippy::too_many_lines)]
    fn from(repr: ServerEventRepr) -> Self {
        match repr {
            ServerEventRepr::Error { event_id, error } => Self::Error { event_id, error },
            ServerEventRepr::SessionCreated { event_id, session } => {
                Self::SessionCreated { event_id, session }
            }
            ServerEventRepr::SessionUpdated { event_id, session } => {
                Self::SessionUpdated { event_id, session }
            }
            ServerEventRepr::ConversationItemCreated {
                event_id,
                previous_item_id,
                item,
            } => Self::ConversationItemCreated {
                event_id,
                previous_item_id,
                item,
            },
            ServerEventRepr::ConversationItemAdded {
                event_id,
                previous_item_id,
                item,
            } => Self::ConversationItemAdded {
                event_id,
                previous_item_id,
                item,
            },
            ServerEventRepr::ConversationItemDone {
                event_id,
                previous_item_id,
                item,
            } => Self::ConversationItemDone {
                event_id,
                previous_item_id,
                item,
            },
            ServerEventRepr::ConversationItemRetrieved { event_id, item } => {
                Self::ConversationItemRetrieved { event_id, item }
            }
            ServerEventRepr::ConversationItemDeleted { event_id, item_id } => {
                Self::ConversationItemDeleted { event_id, item_id }
            }
            ServerEventRepr::ConversationItemTruncated {
                event_id,
                item_id,
                content_index,
                audio_end_ms,
            } => Self::ConversationItemTruncated {
                event_id,
                item_id,
                content_index,
                audio_end_ms,
            },
            ServerEventRepr::InputAudioBufferCommitted {
                event_id,
                previous_item_id,
                item_id,
            } => Self::InputAudioBufferCommitted {
                event_id,
                previous_item_id,
                item_id,
            },
            ServerEventRepr::InputAudioBufferCleared { event_id } => {
                Self::InputAudioBufferCleared { event_id }
            }
            ServerEventRepr::InputAudioBufferSpeechStarted {
                event_id,
                audio_start_ms,
                item_id,
            } => Self::InputAudioBufferSpeechStarted {
                event_id,
                audio_start_ms,
                item_id,
            },
            ServerEventRepr::InputAudioBufferSpeechStopped {
                event_id,
                audio_end_ms,
                item_id,
            } => Self::InputAudioBufferSpeechStopped {
                event_id,
                audio_end_ms,
                item_id,
            },
            ServerEventRepr::InputAudioBufferTimeoutTriggered {
                event_id,
                item_id,
                audio_start_ms,
                audio_end_ms,
            } => Self::InputAudioBufferTimeoutTriggered {
                event_id,
                item_id,
                audio_start_ms,
                audio_end_ms,
            },
            ServerEventRepr::OutputAudioBufferStarted {
                event_id,
                response_id,
            } => Self::OutputAudioBufferStarted {
                event_id,
                response_id,
            },
            ServerEventRepr::OutputAudioBufferStopped {
                event_id,
                response_id,
            } => Self::OutputAudioBufferStopped {
                event_id,
                response_id,
            },
            ServerEventRepr::OutputAudioBufferCleared {
                event_id,
                response_id,
            } => Self::OutputAudioBufferCleared {
                event_id,
                response_id,
            },
            ServerEventRepr::InputAudioTranscriptionDelta {
                event_id,
                item_id,
                content_index,
                delta,
                obfuscation,
                logprobs,
            } => Self::InputAudioTranscriptionDelta {
                event_id,
                item_id,
                content_index,
                delta,
                obfuscation,
                logprobs,
            },
            ServerEventRepr::InputAudioTranscriptionSegment {
                event_id,
                item_id,
                content_index,
                text,
                id,
                speaker,
                start,
                end,
            } => Self::InputAudioTranscriptionSegment {
                event_id,
                item_id,
                content_index,
                text,
                id,
                speaker,
                start,
                end,
            },
            ServerEventRepr::InputAudioTranscriptionFailed {
                event_id,
                item_id,
                content_index,
                error,
            } => Self::InputAudioTranscriptionFailed {
                event_id,
                item_id,
                content_index,
                error,
            },
            ServerEventRepr::InputAudioTranscriptionCompleted {
                event_id,
                item_id,
                content_index,
                transcript,
                logprobs,
                usage,
            } => Self::InputAudioTranscriptionCompleted {
                event_id,
                item_id,
                content_index,
                transcript,
                logprobs,
                usage,
            },
            ServerEventRepr::McpListToolsInProgress { event_id, item_id } => {
                Self::McpListToolsInProgress { event_id, item_id }
            }
            ServerEventRepr::McpListToolsCompleted { event_id, item_id } => {
                Self::McpListToolsCompleted { event_id, item_id }
            }
            ServerEventRepr::McpListToolsFailed {
                event_id,
                item_id,
                error,
            } => Self::McpListToolsFailed {
                event_id,
                item_id,
                error,
            },
            ServerEventRepr::ResponseCreated { event_id, response } => {
                Self::ResponseCreated { event_id, response }
            }
            ServerEventRepr::ResponseDone { event_id, response } => {
                Self::ResponseDone { event_id, response }
            }
            ServerEventRepr::ResponseCancelled { event_id, response } => {
                Self::ResponseCancelled { event_id, response }
            }
            ServerEventRepr::ResponseOutputItemAdded {
                event_id,
                response_id,
                output_index,
                item,
            } => Self::ResponseOutputItemAdded {
                event_id,
                response_id,
                output_index,
                item,
            },
            ServerEventRepr::ResponseOutputItemDone {
                event_id,
                response_id,
                output_index,
                item,
            } => Self::ResponseOutputItemDone {
                event_id,
                response_id,
                output_index,
                item,
            },
            ServerEventRepr::ResponseContentPartAdded {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                part,
            } => Self::ResponseContentPartAdded {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                part,
            },
            ServerEventRepr::ResponseContentPartDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                part,
            } => Self::ResponseContentPartDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                part,
            },
            ServerEventRepr::ResponseOutputTextDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            } => Self::ResponseOutputTextDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            },
            ServerEventRepr::ResponseOutputTextDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                text,
            } => Self::ResponseOutputTextDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                text,
            },
            ServerEventRepr::ResponseOutputAudioDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            } => Self::ResponseOutputAudioDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            },
            ServerEventRepr::ResponseOutputAudioDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                item,
            } => Self::ResponseOutputAudioDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                item,
            },
            ServerEventRepr::ResponseOutputAudioTranscriptDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            } => Self::ResponseOutputAudioTranscriptDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                delta,
            },
            ServerEventRepr::ResponseOutputAudioTranscriptDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                transcript,
            } => Self::ResponseOutputAudioTranscriptDone {
                event_id,
                response_id,
                item_id,
                output_index,
                content_index,
                transcript,
            },
            ServerEventRepr::ResponseFunctionCallArgumentsDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                delta,
            } => Self::ResponseFunctionCallArgumentsDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                delta,
            },
            ServerEventRepr::ResponseFunctionCallArgumentsDone {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                name,
                arguments,
            } => Self::ResponseFunctionCallArgumentsDone {
                event_id,
                response_id,
                item_id,
                output_index,
                call_id,
                name,
                arguments,
            },
            ServerEventRepr::ResponseMcpCallArgumentsDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                delta,
                obfuscation,
            } => Self::ResponseMcpCallArgumentsDelta {
                event_id,
                response_id,
                item_id,
                output_index,
                delta,
                obfuscation,
            },
            ServerEventRepr::ResponseMcpCallArgumentsDone {
                event_id,
                response_id,
                item_id,
                output_index,
                arguments,
            } => Self::ResponseMcpCallArgumentsDone {
                event_id,
                response_id,
                item_id,
                output_index,
                arguments,
            },
            ServerEventRepr::ResponseMcpCallInProgress {
                event_id,
                item_id,
                output_index,
            } => Self::ResponseMcpCallInProgress {
                event_id,
                item_id,
                output_index,
            },
            ServerEventRepr::ResponseMcpCallCompleted {
                event_id,
                item_id,
                output_index,
            } => Self::ResponseMcpCallCompleted {
                event_id,
                item_id,
                output_index,
            },
            ServerEventRepr::ResponseMcpCallFailed {
                event_id,
                item_id,
                output_index,
            } => Self::ResponseMcpCallFailed {
                event_id,
                item_id,
                output_index,
            },
            ServerEventRepr::RateLimitsUpdated {
                event_id,
                rate_limits,
            } => Self::RateLimitsUpdated {
                event_id,
                rate_limits,
            },
            ServerEventRepr::DtmfEventReceived { received_at, event } => {
                Self::DtmfEventReceived { received_at, event }
            }
        }
    }
}

impl Serialize for ServerEvent {
    #[allow(clippy::too_many_lines)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Self::Unknown(value) = self {
            value.serialize(serializer)
        } else {
            let repr = match self {
                Self::Error { event_id, error } => ServerEventRepr::Error {
                    event_id: event_id.clone(),
                    error: error.clone(),
                },
                Self::SessionCreated { event_id, session } => ServerEventRepr::SessionCreated {
                    event_id: event_id.clone(),
                    session: session.clone(),
                },
                Self::SessionUpdated { event_id, session } => ServerEventRepr::SessionUpdated {
                    event_id: event_id.clone(),
                    session: session.clone(),
                },
                Self::ConversationItemCreated {
                    event_id,
                    previous_item_id,
                    item,
                } => ServerEventRepr::ConversationItemCreated {
                    event_id: event_id.clone(),
                    previous_item_id: previous_item_id.clone(),
                    item: item.clone(),
                },
                Self::ConversationItemAdded {
                    event_id,
                    previous_item_id,
                    item,
                } => ServerEventRepr::ConversationItemAdded {
                    event_id: event_id.clone(),
                    previous_item_id: previous_item_id.clone(),
                    item: item.clone(),
                },
                Self::ConversationItemDone {
                    event_id,
                    previous_item_id,
                    item,
                } => ServerEventRepr::ConversationItemDone {
                    event_id: event_id.clone(),
                    previous_item_id: previous_item_id.clone(),
                    item: item.clone(),
                },
                Self::ConversationItemRetrieved { event_id, item } => {
                    ServerEventRepr::ConversationItemRetrieved {
                        event_id: event_id.clone(),
                        item: item.clone(),
                    }
                }
                Self::ConversationItemDeleted { event_id, item_id } => {
                    ServerEventRepr::ConversationItemDeleted {
                        event_id: event_id.clone(),
                        item_id: item_id.clone(),
                    }
                }
                Self::ConversationItemTruncated {
                    event_id,
                    item_id,
                    content_index,
                    audio_end_ms,
                } => ServerEventRepr::ConversationItemTruncated {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    content_index: *content_index,
                    audio_end_ms: *audio_end_ms,
                },
                Self::InputAudioBufferCommitted {
                    event_id,
                    previous_item_id,
                    item_id,
                } => ServerEventRepr::InputAudioBufferCommitted {
                    event_id: event_id.clone(),
                    previous_item_id: previous_item_id.clone(),
                    item_id: item_id.clone(),
                },
                Self::InputAudioBufferCleared { event_id } => {
                    ServerEventRepr::InputAudioBufferCleared {
                        event_id: event_id.clone(),
                    }
                }
                Self::InputAudioBufferSpeechStarted {
                    event_id,
                    audio_start_ms,
                    item_id,
                } => ServerEventRepr::InputAudioBufferSpeechStarted {
                    event_id: event_id.clone(),
                    audio_start_ms: *audio_start_ms,
                    item_id: item_id.clone(),
                },
                Self::InputAudioBufferSpeechStopped {
                    event_id,
                    audio_end_ms,
                    item_id,
                } => ServerEventRepr::InputAudioBufferSpeechStopped {
                    event_id: event_id.clone(),
                    audio_end_ms: *audio_end_ms,
                    item_id: item_id.clone(),
                },
                Self::InputAudioBufferTimeoutTriggered {
                    event_id,
                    item_id,
                    audio_start_ms,
                    audio_end_ms,
                } => ServerEventRepr::InputAudioBufferTimeoutTriggered {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    audio_start_ms: *audio_start_ms,
                    audio_end_ms: *audio_end_ms,
                },
                Self::OutputAudioBufferStarted {
                    event_id,
                    response_id,
                } => ServerEventRepr::OutputAudioBufferStarted {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                },
                Self::OutputAudioBufferStopped {
                    event_id,
                    response_id,
                } => ServerEventRepr::OutputAudioBufferStopped {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                },
                Self::OutputAudioBufferCleared {
                    event_id,
                    response_id,
                } => ServerEventRepr::OutputAudioBufferCleared {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                },
                Self::InputAudioTranscriptionDelta {
                    event_id,
                    item_id,
                    content_index,
                    delta,
                    obfuscation,
                    logprobs,
                } => ServerEventRepr::InputAudioTranscriptionDelta {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    content_index: *content_index,
                    delta: delta.clone(),
                    obfuscation: obfuscation.clone(),
                    logprobs: logprobs.clone(),
                },
                Self::InputAudioTranscriptionSegment {
                    event_id,
                    item_id,
                    content_index,
                    text,
                    id,
                    speaker,
                    start,
                    end,
                } => ServerEventRepr::InputAudioTranscriptionSegment {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    content_index: *content_index,
                    text: text.clone(),
                    id: id.clone(),
                    speaker: speaker.clone(),
                    start: *start,
                    end: *end,
                },
                Self::InputAudioTranscriptionFailed {
                    event_id,
                    item_id,
                    content_index,
                    error,
                } => ServerEventRepr::InputAudioTranscriptionFailed {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    content_index: *content_index,
                    error: error.clone(),
                },
                Self::InputAudioTranscriptionCompleted {
                    event_id,
                    item_id,
                    content_index,
                    transcript,
                    logprobs,
                    usage,
                } => ServerEventRepr::InputAudioTranscriptionCompleted {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    content_index: *content_index,
                    transcript: transcript.clone(),
                    logprobs: logprobs.clone(),
                    usage: usage.clone(),
                },
                Self::McpListToolsInProgress { event_id, item_id } => {
                    ServerEventRepr::McpListToolsInProgress {
                        event_id: event_id.clone(),
                        item_id: item_id.clone(),
                    }
                }
                Self::McpListToolsCompleted { event_id, item_id } => {
                    ServerEventRepr::McpListToolsCompleted {
                        event_id: event_id.clone(),
                        item_id: item_id.clone(),
                    }
                }
                Self::McpListToolsFailed {
                    event_id,
                    item_id,
                    error,
                } => ServerEventRepr::McpListToolsFailed {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    error: error.clone(),
                },
                Self::ResponseCreated { event_id, response } => ServerEventRepr::ResponseCreated {
                    event_id: event_id.clone(),
                    response: response.clone(),
                },
                Self::ResponseDone { event_id, response } => ServerEventRepr::ResponseDone {
                    event_id: event_id.clone(),
                    response: response.clone(),
                },
                Self::ResponseCancelled { event_id, response } => {
                    ServerEventRepr::ResponseCancelled {
                        event_id: event_id.clone(),
                        response: response.clone(),
                    }
                }
                Self::ResponseOutputItemAdded {
                    event_id,
                    response_id,
                    output_index,
                    item,
                } => ServerEventRepr::ResponseOutputItemAdded {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    output_index: *output_index,
                    item: item.clone(),
                },
                Self::ResponseOutputItemDone {
                    event_id,
                    response_id,
                    output_index,
                    item,
                } => ServerEventRepr::ResponseOutputItemDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    output_index: *output_index,
                    item: item.clone(),
                },
                Self::ResponseContentPartAdded {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    part,
                } => ServerEventRepr::ResponseContentPartAdded {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    part: part.clone(),
                },
                Self::ResponseContentPartDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    part,
                } => ServerEventRepr::ResponseContentPartDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    part: part.clone(),
                },
                Self::ResponseOutputTextDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    delta,
                } => ServerEventRepr::ResponseOutputTextDelta {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    delta: delta.clone(),
                },
                Self::ResponseOutputTextDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    text,
                } => ServerEventRepr::ResponseOutputTextDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    text: text.clone(),
                },
                Self::ResponseOutputAudioDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    delta,
                } => ServerEventRepr::ResponseOutputAudioDelta {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    delta: delta.clone(),
                },
                Self::ResponseOutputAudioDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    item,
                } => ServerEventRepr::ResponseOutputAudioDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    item: item.clone(),
                },
                Self::ResponseOutputAudioTranscriptDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    delta,
                } => ServerEventRepr::ResponseOutputAudioTranscriptDelta {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    delta: delta.clone(),
                },
                Self::ResponseOutputAudioTranscriptDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    content_index,
                    transcript,
                } => ServerEventRepr::ResponseOutputAudioTranscriptDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    content_index: *content_index,
                    transcript: transcript.clone(),
                },
                Self::ResponseFunctionCallArgumentsDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    call_id,
                    delta,
                } => ServerEventRepr::ResponseFunctionCallArgumentsDelta {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    call_id: call_id.clone(),
                    delta: delta.clone(),
                },
                Self::ResponseFunctionCallArgumentsDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    call_id,
                    name,
                    arguments,
                } => ServerEventRepr::ResponseFunctionCallArgumentsDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                },
                Self::ResponseMcpCallArgumentsDelta {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    delta,
                    obfuscation,
                } => ServerEventRepr::ResponseMcpCallArgumentsDelta {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    delta: delta.clone(),
                    obfuscation: obfuscation.clone(),
                },
                Self::ResponseMcpCallArgumentsDone {
                    event_id,
                    response_id,
                    item_id,
                    output_index,
                    arguments,
                } => ServerEventRepr::ResponseMcpCallArgumentsDone {
                    event_id: event_id.clone(),
                    response_id: response_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                    arguments: arguments.clone(),
                },
                Self::ResponseMcpCallInProgress {
                    event_id,
                    item_id,
                    output_index,
                } => ServerEventRepr::ResponseMcpCallInProgress {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                },
                Self::ResponseMcpCallCompleted {
                    event_id,
                    item_id,
                    output_index,
                } => ServerEventRepr::ResponseMcpCallCompleted {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                },
                Self::ResponseMcpCallFailed {
                    event_id,
                    item_id,
                    output_index,
                } => ServerEventRepr::ResponseMcpCallFailed {
                    event_id: event_id.clone(),
                    item_id: item_id.clone(),
                    output_index: *output_index,
                },
                Self::RateLimitsUpdated {
                    event_id,
                    rate_limits,
                } => ServerEventRepr::RateLimitsUpdated {
                    event_id: event_id.clone(),
                    rate_limits: rate_limits.clone(),
                },
                Self::DtmfEventReceived { received_at, event } => {
                    ServerEventRepr::DtmfEventReceived {
                        received_at: *received_at,
                        event: event.clone(),
                    }
                }
                Self::Unknown(_) => unreachable!("handled above"),
            };
            repr.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for ServerEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = ArbitraryJson::deserialize(deserializer)?;
        match ServerEventRepr::deserialize(value.clone()) {
            Ok(repr) => Ok(repr.into()),
            Err(err) => {
                tracing::debug!("Failed to parse ServerEvent: {err}");
                Ok(Self::Unknown(value))
            }
        }
    }
}

impl ServerEvent {
    #[must_use]
    pub fn event_id(&self) -> Option<&str> {
        macro_rules! extract {
            ($($variant:ident),*) => {
                match self {
                    $(Self::$variant { event_id, .. } => Some(event_id.as_str()),)*
                    Self::DtmfEventReceived { .. } => None,
                    Self::Unknown(value) => value.get("event_id").and_then(|v| v.as_str()),
                }
            };
        }
        extract!(
            Error,
            SessionCreated,
            SessionUpdated,
            ConversationItemCreated,
            ConversationItemAdded,
            ConversationItemDone,
            ConversationItemRetrieved,
            ConversationItemDeleted,
            ConversationItemTruncated,
            InputAudioBufferCommitted,
            InputAudioBufferCleared,
            InputAudioBufferSpeechStarted,
            InputAudioBufferSpeechStopped,
            InputAudioBufferTimeoutTriggered,
            OutputAudioBufferStarted,
            OutputAudioBufferStopped,
            OutputAudioBufferCleared,
            InputAudioTranscriptionDelta,
            InputAudioTranscriptionSegment,
            InputAudioTranscriptionFailed,
            InputAudioTranscriptionCompleted,
            McpListToolsInProgress,
            McpListToolsCompleted,
            McpListToolsFailed,
            ResponseCreated,
            ResponseDone,
            ResponseCancelled,
            ResponseOutputItemAdded,
            ResponseOutputItemDone,
            ResponseContentPartAdded,
            ResponseContentPartDone,
            ResponseOutputTextDelta,
            ResponseOutputTextDone,
            ResponseOutputAudioDelta,
            ResponseOutputAudioDone,
            ResponseOutputAudioTranscriptDelta,
            ResponseOutputAudioTranscriptDone,
            ResponseFunctionCallArgumentsDelta,
            ResponseFunctionCallArgumentsDone,
            ResponseMcpCallArgumentsDelta,
            ResponseMcpCallArgumentsDone,
            ResponseMcpCallInProgress,
            ResponseMcpCallCompleted,
            ResponseMcpCallFailed,
            RateLimitsUpdated
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimit {
    pub name: String,
    pub limit: u32,
    pub remaining: u32,
    pub reset_seconds: f32,
}
