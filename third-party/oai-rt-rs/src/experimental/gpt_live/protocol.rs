// JSON values implement equality, but these wire structs intentionally avoid
// promising total semantic equality as part of the experimental API.
#![allow(clippy::derive_partial_eq_without_eq)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// Unknown fields retained for forward-compatible round trips.
pub type ExtraFields = Map<String, Value>;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProviderCallId(String);

impl ProviderCallId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderCallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderCallId(<redacted>)")
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateCallRequest {
    pub sdp: String,
    pub session: CallSession,
}

impl fmt::Debug for CreateCallRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateCallRequest")
            .field("sdp", &"<redacted>")
            .field("session", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct CallSession {
    pub model: String,
    pub audio: SessionAudio,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegation: Option<Delegation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionAudio {
    pub output: SessionAudioOutput,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionAudioOutput {
    pub voice: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Delegation {
    Client(ClientDelegation),
    Responses(ResponsesDelegation),
}

impl fmt::Debug for Delegation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Delegation")
            .field(
                "kind",
                &match self {
                    Self::Client(_) => "client",
                    Self::Responses(_) => "responses",
                },
            )
            .field("configuration", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClientDelegationType {
    Client,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientDelegation {
    #[serde(rename = "type")]
    delegation_type: ClientDelegationType,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl ClientDelegation {
    #[must_use]
    pub const fn new(extra: ExtraFields) -> Self {
        Self {
            delegation_type: ClientDelegationType::Client,
            extra,
        }
    }
}

impl Default for ClientDelegation {
    fn default() -> Self {
        Self::new(ExtraFields::new())
    }
}

impl fmt::Debug for ClientDelegation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientDelegation")
            .field("delegation_type", &"client")
            .field("extra_fields", &self.extra.len())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ResponsesDelegationType {
    Responses,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponsesDelegation {
    #[serde(rename = "type")]
    delegation_type: ResponsesDelegationType,
    pub responses: ResponsesConfig,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl ResponsesDelegation {
    #[must_use]
    pub const fn new(responses: ResponsesConfig, extra: ExtraFields) -> Self {
        Self {
            delegation_type: ResponsesDelegationType::Responses,
            responses,
            extra,
        }
    }
}

impl fmt::Debug for ResponsesDelegation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponsesDelegation")
            .field("delegation_type", &"responses")
            .field("responses", &self.responses)
            .field("extra_fields", &self.extra.len())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponsesConfig {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub tools: Vec<FunctionTool>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for ResponsesConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponsesConfig")
            .field("model", &"<redacted>")
            .field(
                "instructions",
                &self.instructions.as_ref().map(|_| "<redacted>"),
            )
            .field("tool_count", &self.tools.len())
            .field("extra_fields", &self.extra.len())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FunctionToolType {
    Function,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionTool {
    #[serde(rename = "type")]
    tool_type: FunctionToolType,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl FunctionTool {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        extra: ExtraFields,
    ) -> Self {
        Self {
            tool_type: FunctionToolType::Function,
            name: name.into(),
            description: description.into(),
            parameters,
            extra,
        }
    }
}

impl fmt::Debug for FunctionTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionTool")
            .field("tool_type", &"function")
            .field("name", &"<redacted>")
            .field("description", &"<redacted>")
            .field("parameters", &"<redacted>")
            .field("extra_fields", &self.extra.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BridgeArguments {
    pub(crate) request: String,
}

impl BridgeArguments {
    #[must_use]
    pub fn request(&self) -> &str {
        &self.request
    }
}

impl fmt::Debug for BridgeArguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeArguments")
            .field("request", &"<redacted>")
            .finish()
    }
}

pub struct CreateCallResponse {
    pub answer_sdp: String,
    pub call_id: ProviderCallId,
}

impl fmt::Debug for CreateCallResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateCallResponse")
            .field("answer_sdp", &"<redacted>")
            .field("call_id", &self.call_id)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextChannel {
    Speakable,
    Commentary,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct InputTextContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for InputTextContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InputTextContent")
            .field("content_type", &self.content_type)
            .field("text", &"<redacted>")
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub enum ClientEvent {
    SessionContextAppend(SessionContextAppend),
    DelegationContextAppend(DelegationContextAppend),
    DelegationFunctionCallOutput(DelegationFunctionCallOutput),
}

impl ClientEvent {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionContextAppend(_) => "session.context.append",
            Self::DelegationContextAppend(_) => "delegation.context.append",
            Self::DelegationFunctionCallOutput(_) => "delegation.function_call_output.create",
        }
    }
}

impl fmt::Debug for ClientEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientEvent")
            .field("kind", &self.kind())
            .field("payload", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FunctionCallId(String);

impl FunctionCallId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for FunctionCallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FunctionCallId(<redacted>)")
    }
}

impl Serialize for FunctionCallId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for FunctionCallId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FunctionCallOutputType {
    FunctionCallOutput,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCallOutput {
    #[serde(rename = "type")]
    output_type: FunctionCallOutputType,
    pub call_id: FunctionCallId,
    pub output: String,
}

impl FunctionCallOutput {
    #[must_use]
    pub fn new(call_id: FunctionCallId, output: impl Into<String>) -> Self {
        Self {
            output_type: FunctionCallOutputType::FunctionCallOutput,
            call_id,
            output: output.into(),
        }
    }
}

impl fmt::Debug for FunctionCallOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionCallOutput")
            .field("output_type", &"function_call_output")
            .field("call_id", &self.call_id)
            .field("output", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct DelegationFunctionCallOutput {
    pub item: FunctionCallOutput,
}

impl DelegationFunctionCallOutput {
    #[must_use]
    pub const fn new(item: FunctionCallOutput) -> Self {
        Self { item }
    }
}

impl fmt::Debug for DelegationFunctionCallOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegationFunctionCallOutput")
            .field("item", &self.item)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionContextAppend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<ContextChannel>,
    pub content: Vec<InputTextContent>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for SessionContextAppend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionContextAppend")
            .field("channel", &self.channel)
            .field("content", &self.content)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct DelegationContextAppend {
    pub delegation_item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<ContextChannel>,
    pub content: Vec<InputTextContent>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for DelegationContextAppend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegationContextAppend")
            .field("delegation_item_id", &"<redacted>")
            .field("channel", &self.channel)
            .field("content", &self.content)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub enum ServerEvent {
    SessionStarted(SessionStarted),
    SessionContextAppended(SessionContextAppended),
    InputTranscriptAdded(InputTranscriptAdded),
    OutputTranscriptAdded(OutputTranscriptAdded),
    TurnCreated(TurnCreated),
    TurnDelta(TurnDelta),
    TurnDone(TurnDone),
    DelegationCreated(DelegationCreated),
    DelegationContextAppended(DelegationContextAppended),
    Unknown(UnknownEvent),
}

impl fmt::Debug for ServerEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if matches!(self, Self::Unknown(_)) {
            "unknown"
        } else {
            self.kind()
        };
        formatter
            .debug_struct("ServerEvent")
            .field("kind", &kind)
            .field("payload", &"<redacted>")
            .finish()
    }
}

impl ServerEvent {
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::SessionStarted(_) => "session.started",
            Self::SessionContextAppended(_) => "session.context.appended",
            Self::InputTranscriptAdded(_) => "input_transcript.added",
            Self::OutputTranscriptAdded(_) => "output_transcript.added",
            Self::TurnCreated(_) => "turn.created",
            Self::TurnDelta(_) => "turn.delta",
            Self::TurnDone(_) => "turn.done",
            Self::DelegationCreated(_) => "delegation.created",
            Self::DelegationContextAppended(_) => "delegation.context.appended",
            Self::Unknown(event) => event.kind(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCarrier {
    Sideband,
    OrderedOaiEvents,
}

#[derive(Clone, PartialEq)]
pub struct ReceivedServerEvent {
    carrier: EventCarrier,
    byte_count: usize,
    event: ServerEvent,
}

impl ReceivedServerEvent {
    pub(crate) const fn new(carrier: EventCarrier, byte_count: usize, event: ServerEvent) -> Self {
        Self {
            carrier,
            byte_count,
            event,
        }
    }

    #[must_use]
    pub const fn carrier(&self) -> EventCarrier {
        self.carrier
    }

    #[must_use]
    pub const fn byte_count(&self) -> usize {
        self.byte_count
    }

    #[must_use]
    pub const fn event(&self) -> &ServerEvent {
        &self.event
    }

    #[must_use]
    pub fn into_event(self) -> ServerEvent {
        self.event
    }
}

impl fmt::Debug for ReceivedServerEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceivedServerEvent")
            .field("carrier", &self.carrier)
            .field("byte_count", &self.byte_count)
            .field(
                "kind",
                &if matches!(self.event, ServerEvent::Unknown(_)) {
                    "unknown"
                } else {
                    self.event.kind()
                },
            )
            .field("payload", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct UnknownEvent {
    kind: String,
    raw: Value,
}

impl UnknownEvent {
    pub(crate) const fn new(kind: String, raw: Value) -> Self {
        Self { kind, raw }
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub const fn raw(&self) -> &Value {
        &self.raw
    }
}

impl fmt::Debug for UnknownEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnknownEvent")
            .field("kind", &"<unrecognized>")
            .field("raw", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionStarted {
    pub session: StartedSession,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct StartedSession {
    pub id: String,
    pub expires_at: u64,
    pub status: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionContextAppended {
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct InputTranscriptAdded {
    pub start_ms: u64,
    pub end_ms: u64,
    pub item: TranscriptItem,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputTranscriptAdded {
    pub start_ms: u64,
    pub end_ms: u64,
    pub item: TranscriptItem,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub text: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for TranscriptItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TranscriptItem")
            .field("id", &"<redacted>")
            .field("item_type", &self.item_type)
            .field("text", &"<redacted>")
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnCreated {
    pub turn: Turn,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Turn {
    pub id: String,
    pub role: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub transcript: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for Turn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Turn")
            .field("id", &"<redacted>")
            .field("role", &self.role)
            .field("start_ms", &self.start_ms)
            .field("end_ms", &self.end_ms)
            .field("transcript", &"<redacted>")
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnDelta {
    pub turn_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub delta: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for TurnDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TurnDelta")
            .field("turn_id", &"<redacted>")
            .field("start_ms", &self.start_ms)
            .field("end_ms", &self.end_ms)
            .field("delta", &"<redacted>")
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnDone {
    pub turn: Turn,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct DelegationCreated {
    pub offset_ms: u64,
    pub item: DelegationItem,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct DelegationItem {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub target: String,
    pub handoff_id: String,
    pub user_bidi_turn_id: String,
    pub content: Vec<Value>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for DelegationItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegationItem")
            .field("id", &"<redacted>")
            .field("item_type", &self.item_type)
            .field("target", &self.target)
            .field("handoff_id", &"<redacted>")
            .field("user_bidi_turn_id", &"<redacted>")
            .field("content_count", &self.content.len())
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct DelegationContextAppended {
    pub delegation_item_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

impl fmt::Debug for DelegationContextAppended {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegationContextAppended")
            .field("delegation_item_id", &"<redacted>")
            .field("start_ms", &self.start_ms)
            .field("end_ms", &self.end_ms)
            .field("extra_keys", &self.extra.keys().collect::<Vec<_>>())
            .finish()
    }
}
