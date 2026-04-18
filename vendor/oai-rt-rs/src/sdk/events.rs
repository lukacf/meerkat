use crate::error::ServerError;
use crate::protocol::models::{ContentPart, Item, Usage};
use crate::protocol::server_events::ServerEvent;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum SdkEvent {
    TextDelta {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    TextDone {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        text: String,
    },
    AudioDelta {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    AudioDone {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        item: Option<Box<Item>>,
    },
    TranscriptDelta {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    TranscriptDone {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        transcript: String,
    },
    ContentPartAdded {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
    },
    ContentPartDone {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: ContentPart,
    },
    ToolCall {
        response_id: String,
        item_id: String,
        output_index: u32,
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolCallDelta {
        response_id: String,
        item_id: String,
        output_index: u32,
        call_id: String,
        delta: String,
    },
    InputTranscriptionDelta {
        item_id: String,
        content_index: u32,
        delta: String,
    },
    InputTranscriptionCompleted {
        item_id: String,
        content_index: u32,
        transcript: String,
        usage: Option<Usage>,
    },
    Error {
        event_id: String,
        error: ServerError,
    },
    Raw(Box<ServerEvent>),
}

pub struct EventStream<'a> {
    rx: &'a mut mpsc::Receiver<SdkEvent>,
}

impl<'a> EventStream<'a> {
    #[must_use]
    pub const fn new(rx: &'a mut mpsc::Receiver<SdkEvent>) -> Self {
        Self { rx }
    }
}

impl Stream for EventStream<'_> {
    type Item = SdkEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.rx).poll_recv(cx)
    }
}

impl SdkEvent {
    #[must_use]
    pub fn from_server(event: ServerEvent) -> Option<Self> {
        let boxed = Box::new(event);
        if let Some(mapped) = map_response_ref(&boxed) {
            return Some(mapped);
        }
        if let Some(mapped) = map_transcription_ref(&boxed) {
            return Some(mapped);
        }
        if let Some(mapped) = map_error_ref(&boxed) {
            return Some(mapped);
        }
        Some(Self::Raw(boxed))
    }
}

fn map_response_ref(event: &ServerEvent) -> Option<SdkEvent> {
    map_response_text(event)
        .or_else(|| map_response_audio(event))
        .or_else(|| map_response_content(event))
        .or_else(|| map_response_tool(event))
}

fn map_response_text(event: &ServerEvent) -> Option<SdkEvent> {
    match event {
        ServerEvent::ResponseOutputTextDelta {
            response_id,
            item_id,
            output_index,
            content_index,
            delta,
            ..
        } => Some(text_delta(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            delta.clone(),
        )),
        ServerEvent::ResponseOutputTextDone {
            response_id,
            item_id,
            output_index,
            content_index,
            text,
            ..
        } => Some(text_done(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            text.clone(),
        )),
        _ => None,
    }
}

fn map_response_audio(event: &ServerEvent) -> Option<SdkEvent> {
    match event {
        ServerEvent::ResponseOutputAudioDelta {
            response_id,
            item_id,
            output_index,
            content_index,
            delta,
            ..
        } => Some(audio_delta(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            delta.clone(),
        )),
        ServerEvent::ResponseOutputAudioDone {
            response_id,
            item_id,
            output_index,
            content_index,
            item,
            ..
        } => Some(audio_done(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            item.clone(),
        )),
        ServerEvent::ResponseOutputAudioTranscriptDelta {
            response_id,
            item_id,
            output_index,
            content_index,
            delta,
            ..
        } => Some(transcript_delta(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            delta.clone(),
        )),
        ServerEvent::ResponseOutputAudioTranscriptDone {
            response_id,
            item_id,
            output_index,
            content_index,
            transcript,
            ..
        } => Some(transcript_done(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            transcript.clone(),
        )),
        _ => None,
    }
}

fn map_response_content(event: &ServerEvent) -> Option<SdkEvent> {
    match event {
        ServerEvent::ResponseContentPartAdded {
            response_id,
            item_id,
            output_index,
            content_index,
            part,
            ..
        } => Some(content_part_added(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            part.clone(),
        )),
        ServerEvent::ResponseContentPartDone {
            response_id,
            item_id,
            output_index,
            content_index,
            part,
            ..
        } => Some(content_part_done(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            *content_index,
            part.clone(),
        )),
        _ => None,
    }
}

fn map_response_tool(event: &ServerEvent) -> Option<SdkEvent> {
    match event {
        ServerEvent::ResponseFunctionCallArgumentsDelta {
            response_id,
            item_id,
            output_index,
            call_id,
            delta,
            ..
        } => Some(tool_call_delta(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            call_id.clone(),
            delta.clone(),
        )),
        ServerEvent::ResponseFunctionCallArgumentsDone {
            response_id,
            item_id,
            output_index,
            call_id,
            name,
            arguments,
            ..
        } => Some(tool_call_done(
            response_id.clone(),
            item_id.clone(),
            *output_index,
            call_id.clone(),
            name.clone(),
            arguments.clone(),
        )),
        _ => None,
    }
}

fn map_transcription_ref(event: &ServerEvent) -> Option<SdkEvent> {
    match event {
        ServerEvent::InputAudioTranscriptionDelta {
            item_id,
            content_index,
            delta,
            ..
        } => Some(input_transcription_delta(
            item_id.clone(),
            *content_index,
            delta.clone(),
        )),
        ServerEvent::InputAudioTranscriptionCompleted {
            item_id,
            content_index,
            transcript,
            usage,
            ..
        } => Some(input_transcription_completed(
            item_id.clone(),
            *content_index,
            transcript.clone(),
            usage.clone(),
        )),
        _ => None,
    }
}

fn map_error_ref(event: &ServerEvent) -> Option<SdkEvent> {
    match event {
        ServerEvent::Error { event_id, error } => {
            Some(error_event(event_id.clone(), error.clone()))
        }
        _ => None,
    }
}

const fn text_delta(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    delta: String,
) -> SdkEvent {
    SdkEvent::TextDelta {
        response_id,
        item_id,
        output_index,
        content_index,
        delta,
    }
}

const fn text_done(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    text: String,
) -> SdkEvent {
    SdkEvent::TextDone {
        response_id,
        item_id,
        output_index,
        content_index,
        text,
    }
}

const fn audio_delta(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    delta: String,
) -> SdkEvent {
    SdkEvent::AudioDelta {
        response_id,
        item_id,
        output_index,
        content_index,
        delta,
    }
}

fn audio_done(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    item: Option<Item>,
) -> SdkEvent {
    SdkEvent::AudioDone {
        response_id,
        item_id,
        output_index,
        content_index,
        item: item.map(Box::new),
    }
}

const fn transcript_delta(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    delta: String,
) -> SdkEvent {
    SdkEvent::TranscriptDelta {
        response_id,
        item_id,
        output_index,
        content_index,
        delta,
    }
}

const fn transcript_done(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    transcript: String,
) -> SdkEvent {
    SdkEvent::TranscriptDone {
        response_id,
        item_id,
        output_index,
        content_index,
        transcript,
    }
}

const fn content_part_added(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    part: ContentPart,
) -> SdkEvent {
    SdkEvent::ContentPartAdded {
        response_id,
        item_id,
        output_index,
        content_index,
        part,
    }
}

const fn content_part_done(
    response_id: String,
    item_id: String,
    output_index: u32,
    content_index: u32,
    part: ContentPart,
) -> SdkEvent {
    SdkEvent::ContentPartDone {
        response_id,
        item_id,
        output_index,
        content_index,
        part,
    }
}

const fn tool_call_delta(
    response_id: String,
    item_id: String,
    output_index: u32,
    call_id: String,
    delta: String,
) -> SdkEvent {
    SdkEvent::ToolCallDelta {
        response_id,
        item_id,
        output_index,
        call_id,
        delta,
    }
}

const fn tool_call_done(
    response_id: String,
    item_id: String,
    output_index: u32,
    call_id: String,
    name: String,
    arguments: String,
) -> SdkEvent {
    SdkEvent::ToolCall {
        response_id,
        item_id,
        output_index,
        call_id,
        name,
        arguments,
    }
}

const fn input_transcription_delta(item_id: String, content_index: u32, delta: String) -> SdkEvent {
    SdkEvent::InputTranscriptionDelta {
        item_id,
        content_index,
        delta,
    }
}

const fn input_transcription_completed(
    item_id: String,
    content_index: u32,
    transcript: String,
    usage: Option<Usage>,
) -> SdkEvent {
    SdkEvent::InputTranscriptionCompleted {
        item_id,
        content_index,
        transcript,
        usage,
    }
}

const fn error_event(event_id: String, error: ServerError) -> SdkEvent {
    SdkEvent::Error { event_id, error }
}
