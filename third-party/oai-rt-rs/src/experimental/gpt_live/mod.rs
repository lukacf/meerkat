//! Mechanical support for the private, pre-release GPT Live wire protocol.
//!
//! This module deliberately does not select models, resolve OAuth credentials,
//! assign application identities, or interpret transcript and delegation
//! semantics. Callers retain those responsibilities.

mod codec;
mod protocol;
mod redaction;
mod transport;

pub use codec::{
    CodecError, MAX_BRIDGE_ARGUMENT_BYTES, MAX_FUNCTION_OUTPUT_BYTES, MAX_RAW_JSON_EVENT_BYTES,
    decode_bridge_arguments, decode_received_server_event, decode_server_event,
    encode_client_event, encode_server_event,
};
pub use protocol::{
    BridgeArguments, CallSession, ClientDelegation, ClientEvent, ContextChannel, CreateCallRequest,
    CreateCallResponse, Delegation, DelegationContextAppend, DelegationContextAppended,
    DelegationCreated, DelegationFunctionCallOutput, DelegationItem, EventCarrier, ExtraFields,
    FunctionCallId, FunctionCallOutput, FunctionTool, InputTextContent, InputTranscriptAdded,
    ProviderCallId, ReceivedServerEvent, ResponsesConfig, ResponsesDelegation, ServerEvent,
    SessionAudio, SessionAudioOutput, SessionContextAppend, SessionContextAppended, SessionStarted,
    StartedSession, TranscriptItem, Turn, TurnCreated, TurnDelta, TurnDone, UnknownEvent,
};
pub use redaction::{Direction, TerminalClass, WireSummary};
pub use transport::{
    GptLiveCredentials, GptLiveEndpoints, GptLiveTransport, HttpFailureClass, Sideband,
    SidebandHeaders, SidebandReceiver, SidebandSender, TransportError, WebSocketFailureClass,
};
