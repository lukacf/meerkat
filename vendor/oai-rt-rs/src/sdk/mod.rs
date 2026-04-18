//! High-level SDK facade over the Realtime protocol.
//!
//! The SDK exposes a simple async callback interface while keeping the low-level
//! protocol types accessible through `crate::protocol` when you need full control.

mod builder;
pub mod events;
mod handlers;
mod response;
mod session;
mod tools;
mod transport;
mod voice;

pub use builder::{Realtime, RealtimeBuilder, VoiceSessionBuilder};
pub use events::{EventStream, SdkEvent};
pub use handlers::{EventHandlers, RawEventHandler, TextHandler, ToolCallHandler};
pub use response::ResponseBuilder;
pub use session::AudioIn;
pub use session::{Session, SessionHandle};
pub use tools::{
    BoxFuture as ToolFuture, ToolCall, ToolDefinition, ToolRegistry, ToolResult, ToolSpec,
};
pub use voice::{AudioChunk, TranscriptChunk, VoiceEvent, VoiceEventStream};
