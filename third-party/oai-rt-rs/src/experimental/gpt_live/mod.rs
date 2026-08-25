//! Mechanical support for the private, pre-release GPT Live wire protocol.
//!
//! This module deliberately does not select models, resolve OAuth credentials,
//! assign application identities, or interpret transcript and delegation
//! semantics. Callers retain those responsibilities.

mod codec;
mod protocol;
mod redaction;
mod transport;

pub use codec::{CodecError, decode_server_event, encode_client_event, encode_server_event};
pub use protocol::{
    CallSession, ClientDelegation, ClientEvent, ContextChannel, CreateCallRequest,
    CreateCallResponse, DelegationContextAppend, DelegationContextAppended, DelegationCreated,
    DelegationItem, ExtraFields, InputTextContent, InputTranscriptAdded, ProviderCallId,
    ServerEvent, SessionAudio, SessionAudioOutput, SessionContextAppend, SessionContextAppended,
    SessionStarted, StartedSession, TranscriptItem, Turn, TurnCreated, TurnDelta, TurnDone,
    UnknownEvent,
};
pub use redaction::{Direction, TerminalClass, WireSummary};
pub use transport::{
    GptLiveCredentials, GptLiveEndpoints, GptLiveTransport, HttpFailureClass, Sideband,
    SidebandHeaders, SidebandReceiver, SidebandSender, TransportError, WebSocketFailureClass,
};
