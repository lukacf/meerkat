//! Realtime channel wire contracts.

use serde::{Deserialize, Serialize};

/// Target for a public realtime channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeChannelTarget {
    SessionTarget {
        session_id: String,
    },
    MobMemberTarget {
        mob_id: String,
        agent_identity: String,
    },
}

/// Opening role for a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeChannelRole {
    Primary,
    Observer,
}

/// Turning mode for a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTurningMode {
    ProviderManaged,
    ExplicitCommit,
}

/// Input modality kind supported by a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeInputKind {
    Text,
    Audio,
    Video,
}

/// Output modality kind supported by a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeOutputKind {
    Text,
    Audio,
    Video,
}

/// Public reconnect policy for a realtime channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeReconnectPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub max_total_ms: u64,
}

/// Product-facing realtime capability set for one target/provider combination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_kinds: Vec<RealtimeInputKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_kinds: Vec<RealtimeOutputKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turning_modes: Vec<RealtimeTurningMode>,
    pub interrupt_supported: bool,
    pub transcript_supported: bool,
    pub tool_lifecycle_events_supported: bool,
    pub video_supported: bool,
}

/// Lifecycle state for a realtime channel.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RealtimeChannelState {
    Opening,
    Ready,
    Interrupted,
    Reconnecting,
    Closed,
    Error,
}

/// Public realtime channel status projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelStatus {
    pub state: RealtimeChannelState,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Request payload for `realtime/open_info`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeOpenRequest {
    pub target: RealtimeChannelTarget,
    pub role: RealtimeChannelRole,
    pub turning_mode: RealtimeTurningMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_policy: Option<RealtimeReconnectPolicy>,
}

/// Response payload for `realtime/open_info`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeOpenInfo {
    pub ws_url: String,
    pub open_token: String,
    pub expires_at: String,
    pub target: RealtimeChannelTarget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_protocol_versions: Vec<String>,
    pub default_protocol_version: String,
    pub capabilities: RealtimeCapabilities,
}

/// Request payload for `realtime/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeStatusParams {
    pub target: RealtimeChannelTarget,
}

/// Response payload for `realtime/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeStatusResult {
    pub status: RealtimeChannelStatus,
}

/// Request payload for `realtime/capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeCapabilitiesParams {
    pub target: RealtimeChannelTarget,
}

/// Response payload for `realtime/capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeCapabilitiesResult {
    pub capabilities: RealtimeCapabilities,
}

/// A text chunk for realtime ingress/egress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeTextChunk {
    pub text: String,
}

/// A text delta chunk for realtime output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeTextDelta {
    pub delta: String,
}

/// An opaque realtime audio chunk with MIME metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeAudioChunk {
    pub mime_type: String,
    pub data: String,
}

/// An opaque realtime video chunk with MIME metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeVideoChunk {
    pub mime_type: String,
    pub data: String,
}

/// Modality-neutral input chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeInputChunk {
    TextChunk(RealtimeTextChunk),
    AudioChunk(RealtimeAudioChunk),
    VideoChunk(RealtimeVideoChunk),
}

/// Modality-neutral output chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeOutputChunk {
    TextDelta(RealtimeTextDelta),
    AudioChunk(RealtimeAudioChunk),
    VideoChunk(RealtimeVideoChunk),
}

/// Normalized realtime event stream payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeEvent {
    InputTranscriptPartial { text: String },
    InputTranscriptFinal { text: String },
    TurnStarted,
    TurnCommitted,
    TurnCompleted,
    OutputTextDelta { delta: String },
    OutputAudioChunk { chunk: RealtimeAudioChunk },
    OutputVideoChunk { chunk: RealtimeVideoChunk },
    Interrupted,
    ToolCallRequested { call_id: String, tool_name: String },
    ToolCallCompleted { call_id: String },
    ToolCallFailed { call_id: String, error: String },
    StatusChanged { status: RealtimeChannelStatus },
    NeedsReattach,
}

/// Payload for `channel.open`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelOpenFrame {
    pub protocol_version: String,
    pub open_token: String,
    pub role: RealtimeChannelRole,
    pub turning_mode: RealtimeTurningMode,
}

/// Payload for `channel.input`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelInputFrame {
    pub chunk: RealtimeInputChunk,
}

/// Payload for `channel.opened`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelOpenedFrame {
    pub protocol_version: String,
    pub status: RealtimeChannelStatus,
    pub capabilities: RealtimeCapabilities,
    pub role: RealtimeChannelRole,
}

/// Payload for `channel.status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelStatusFrame {
    pub status: RealtimeChannelStatus,
}

/// Payload for `channel.event`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelEventFrame {
    pub event: RealtimeEvent,
}

/// Payload for `channel.error`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelErrorFrame {
    pub code: String,
    pub message: String,
}

/// Payload for `channel.closed`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealtimeChannelClosedFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Client-to-server realtime frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
pub enum RealtimeClientFrame {
    #[serde(rename = "channel.open")]
    ChannelOpen(RealtimeChannelOpenFrame),
    #[serde(rename = "channel.input")]
    ChannelInput(RealtimeChannelInputFrame),
    #[serde(rename = "channel.commit_turn")]
    ChannelCommitTurn,
    #[serde(rename = "channel.interrupt")]
    ChannelInterrupt,
    #[serde(rename = "channel.close")]
    ChannelClose,
}

/// Server-to-client realtime frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
pub enum RealtimeServerFrame {
    #[serde(rename = "channel.opened")]
    ChannelOpened(RealtimeChannelOpenedFrame),
    #[serde(rename = "channel.status")]
    ChannelStatus(RealtimeChannelStatusFrame),
    #[serde(rename = "channel.event")]
    ChannelEvent(RealtimeChannelEventFrame),
    #[serde(rename = "channel.error")]
    ChannelError(RealtimeChannelErrorFrame),
    #[serde(rename = "channel.closed")]
    ChannelClosed(RealtimeChannelClosedFrame),
}
