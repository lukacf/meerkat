use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    SpeechStarted {
        audio_start_ms: Option<u32>,
    },
    SpeechStopped {
        audio_end_ms: Option<u32>,
    },
    AudioDelta {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
        pcm: Vec<u8>,
    },
    AudioDone {
        response_id: String,
        item_id: String,
        output_index: u32,
        content_index: u32,
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
    UserTranscriptDone {
        item_id: String,
        content_index: u32,
        transcript: String,
    },
    ResponseCreated {
        response_id: String,
    },
    ResponseDone {
        response_id: String,
    },
    ResponseCancelled {
        response_id: String,
    },
    DecodeError {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub response_id: String,
    pub item_id: String,
    pub output_index: u32,
    pub content_index: u32,
    pub pcm: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TranscriptChunk {
    pub response_id: String,
    pub item_id: String,
    pub output_index: u32,
    pub content_index: u32,
    pub text: String,
    pub is_final: bool,
}

pub struct VoiceEventStream<'a> {
    rx: &'a mut mpsc::Receiver<VoiceEvent>,
}

impl<'a> VoiceEventStream<'a> {
    #[must_use]
    pub const fn new(rx: &'a mut mpsc::Receiver<VoiceEvent>) -> Self {
        Self { rx }
    }
}

impl Stream for VoiceEventStream<'_> {
    type Item = VoiceEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.rx).poll_recv(cx)
    }
}
