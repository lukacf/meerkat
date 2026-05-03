//! Runtime and input wire contracts.

use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Serialize};

use crate::wire::session::WireContentBlock;
use meerkat_core::PeerCorrelationId;
use meerkat_core::comms::{PeerId, PeerName};

fn deserialize_raw_json_box<'de, D>(
    deserializer: D,
) -> Result<Box<serde_json::value::RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    serde_json::value::RawValue::from_string(value.to_string()).map_err(serde::de::Error::custom)
}

/// Request payload for `session/realtime_attachment_status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeRealtimeAttachmentStatusParams {
    pub session_id: String,
}

/// Terminal status for dedicated correlated peer-response ingress.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PeerResponseTerminalStatusWire {
    Completed,
    Failed,
    Cancelled,
}

/// Dedicated request payload for `session/peer_response_terminal`.
///
/// Not `PartialEq`: `result` is opaque peer-returned bytes carried as
/// `Box<RawValue>` (pass-through fidelity — the peer is the typed
/// authority over its own payload shape). Allow-listed per
/// `dogma-blind-spots` §7 alongside tool-call args.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SessionPeerResponseTerminalParams {
    pub session_id: String,
    pub peer_id: PeerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<PeerName>,
    pub request_id: PeerCorrelationId,
    pub status: PeerResponseTerminalStatusWire,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub result: Box<serde_json::value::RawValue>,
}

/// Typed event envelope for the generic `session/external_event` and
/// `/sessions/{id}/external-events` surfaces.
///
/// Not `PartialEq`: the `GenericJson.payload` and
/// `PeerResponseTerminal.result` bodies ride as `Box<RawValue>` — opaque
/// caller-supplied JSON that never gets pattern-matched at this layer.
/// Allow-listed per `dogma-blind-spots` §7.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionExternalEventEnvelope {
    /// Generic external JSON event admitted as `Input::ExternalEvent`.
    GenericJson {
        event_type: String,
        #[serde(deserialize_with = "deserialize_raw_json_box")]
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        payload: Box<serde_json::value::RawValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<WireContentBlock>>,
    },
    /// Reserved typed semantic. Callers must use the dedicated
    /// `session/peer_response_terminal` / `/peer-response-terminal` surface
    /// instead of routing terminal peer responses through the generic event
    /// ingress.
    PeerResponseTerminal {
        peer_id: PeerId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<PeerName>,
        request_id: PeerCorrelationId,
        status: PeerResponseTerminalStatusWire,
        #[serde(deserialize_with = "deserialize_raw_json_box")]
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        result: Box<serde_json::value::RawValue>,
    },
}

/// Public runtime state projection used by RPC surfaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRuntimeState {
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

/// Response payload for runtime-backed session status projections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeStateResult {
    pub state: WireRuntimeState,
}

/// Public live attachment status projection used by runtime and mob surfaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRealtimeAttachmentStatus {
    Unattached,
    IntentPresentUnbound,
    BindingNotReady,
    BindingReady,
    ReplacementPending,
    ReattachRequired,
}

/// Response payload for `session/realtime_attachment_status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeRealtimeAttachmentStatusResult {
    pub status: WireRealtimeAttachmentStatus,
}

/// Discriminator for runtime-backed input submission responses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAcceptOutcomeType {
    Accepted,
    Deduplicated,
    Rejected,
}

/// Public input lifecycle state projection used by RPC surfaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireInputLifecycleState {
    Accepted,
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

/// Input transition history entry for RPC-facing snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInputStateHistoryEntry {
    pub timestamp: String,
    pub from: WireInputLifecycleState,
    pub to: WireInputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Typed wire projection of the input admission policy.
///
/// Formerly `serde_json::Value`; wave-b retyped it so callers can pattern
/// match on admission behavior instead of parsing opaque JSON.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireInputPolicy {
    Stage,
    Queue,
    Immediate,
}

/// Typed wire projection of an input's terminal outcome.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireInputTerminalOutcome {
    Completed,
    Abandoned,
    Superseded,
    Coalesced,
    Cancelled,
}

/// Typed wire projection of an input's durability class.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireInputDurability {
    Durable,
    Volatile,
    Ephemeral,
}

/// Typed wire projection of where an input state was reconstructed from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireReconstructionSource {
    Live,
    EventStore,
    Snapshot,
    Replay,
}

/// Typed wire projection of the persisted input carrier.
///
/// The carrier is a structural discriminator. Untypeable provider-specific
/// body bytes should be projected via `StructuredProviderExtension` — a
/// typed opaque-bag newtype explicitly marked non-semantic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WirePersistedInput {
    Prompt {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_preview: Option<String>,
    },
    ExternalEvent {
        event_type: String,
    },
    PeerMessage {
        peer_name: PeerName,
    },
    Continuation,
    Other {
        carrier: String,
    },
}

/// Typed non-semantic opaque bag for wire fields that cannot be fully typed
/// without blocking Wave B. Explicitly marked non-semantic and RMAT-exempt.
///
/// Re-exported from `meerkat_core::lifecycle::run_primitive` so the wire
/// path (`meerkat_contracts::wire::runtime::StructuredProviderExtension`)
/// is preserved for external callers while the canonical definition lives
/// in core (C-1 / adversarial review flaw 5). Core needs to name the bag
/// to project `ProviderTag::Unknown { bag }` from V3 legacy rows without a
/// cross-crate cycle.
pub use meerkat_core::lifecycle::run_primitive::StructuredProviderExtension;

/// RPC-facing input state snapshot.
///
/// All fields are typed. Wave B replaced six former `serde_json::Value`
/// fields with typed projections so the wire carries no untyped carriers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInputState {
    pub input_id: String,
    pub current_state: WireInputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<WireInputPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<WireInputTerminalOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durability: Option<WireInputDurability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub recovery_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<WireInputStateHistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconstruction_source: Option<WireReconstructionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted_input: Option<WirePersistedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_boundary_sequence: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Response payload for runtime-backed input submission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeAcceptResult {
    pub outcome_type: RuntimeAcceptOutcomeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<WireInputPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<WireInputState>,
}

// -----------------------------------------------------------------------
// V8 — Typed replacement for the legacy untyped session ingress method
// (retired name: the `session/accept`-prefixed-`input` RPC; kept without
// the literal string here so `scripts/session_control_public_name_scan.sh`
// doesn't mistake an explanatory retirement note for an active surface).
// -----------------------------------------------------------------------

/// Typed wire projection of `ConversationAppendRole`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireConversationAppendRole {
    User,
    Assistant,
    SystemNotice,
    Tool,
}

/// Typed wire projection of a conversation append operation. Body content
/// is carried as the existing `WireContentBlock` vector so the wire stays
/// structurally typed end-to-end.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireConversationAppend {
    pub role: WireConversationAppendRole,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<WireContentBlock>,
}

/// Typed wire projection of a context-only append.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireConversationContextAppend {
    pub key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<WireContentBlock>,
}

/// Typed wire projection of a batched staged input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireStagedRunInput {
    #[serde(default)]
    pub contributing_input_ids: Vec<String>,
    #[serde(default)]
    pub appends: Vec<WireConversationAppend>,
    #[serde(default)]
    pub context_appends: Vec<WireConversationContextAppend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<WireRuntimeTurnMetadata>,
}

/// Typed wire projection of `meerkat_core::lifecycle::run_primitive::RunPrimitive`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::large_enum_variant)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireRunPrimitive {
    StagedInput(WireStagedRunInput),
    ImmediateAppend(WireConversationAppend),
    ImmediateContextAppend(WireConversationContextAppend),
}

/// Typed request payload for the input-acceptance RPC method.
///
/// Replaces the untyped ad-hoc JSON shape the RPC handler parsed by hand
/// pre-wave-b. Every field is a typed projection of a core type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionAcceptInputParams {
    pub session_id: String,
    pub primitive: WireRunPrimitive,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_metadata: Option<WireRuntimeTurnMetadata>,
}

// -----------------------------------------------------------------------
// V3 — Typed wire projection of `RuntimeTurnMetadata`.
//
// This is the typed projection part of the Wave B turn-metadata rewrite.
// `SessionAcceptInputParams` + the stringly-typed `WireInputState` fields
// above belong to B-9's scope and are intentionally untouched here.
// -----------------------------------------------------------------------

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::KeepAliveMode`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireKeepAliveMode {
    Pinned,
    PolicyDriven,
}

impl From<meerkat_core::lifecycle::run_primitive::KeepAliveMode> for WireKeepAliveMode {
    fn from(value: meerkat_core::lifecycle::run_primitive::KeepAliveMode) -> Self {
        use meerkat_core::lifecycle::run_primitive::KeepAliveMode as Core;
        match value {
            Core::Pinned => Self::Pinned,
            Core::PolicyDriven => Self::PolicyDriven,
        }
    }
}

impl From<WireKeepAliveMode> for meerkat_core::lifecycle::run_primitive::KeepAliveMode {
    fn from(value: WireKeepAliveMode) -> Self {
        match value {
            WireKeepAliveMode::Pinned => Self::Pinned,
            WireKeepAliveMode::PolicyDriven => Self::PolicyDriven,
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::KeepAlivePolicy`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireKeepAlivePolicy {
    pub ttl_secs: u64,
    pub policy: WireKeepAliveMode,
}

impl From<meerkat_core::lifecycle::run_primitive::KeepAlivePolicy> for WireKeepAlivePolicy {
    fn from(value: meerkat_core::lifecycle::run_primitive::KeepAlivePolicy) -> Self {
        Self {
            ttl_secs: value.ttl.as_secs(),
            policy: value.policy.into(),
        }
    }
}

impl From<WireKeepAlivePolicy> for meerkat_core::lifecycle::run_primitive::KeepAlivePolicy {
    fn from(value: WireKeepAlivePolicy) -> Self {
        Self {
            ttl: std::time::Duration::from_secs(value.ttl_secs),
            policy: value.policy.into(),
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::TurnInstructionKind`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTurnInstructionKind {
    User,
    System,
    Host,
}

impl From<meerkat_core::lifecycle::run_primitive::TurnInstructionKind> for WireTurnInstructionKind {
    fn from(value: meerkat_core::lifecycle::run_primitive::TurnInstructionKind) -> Self {
        use meerkat_core::lifecycle::run_primitive::TurnInstructionKind as Core;
        match value {
            Core::User => Self::User,
            Core::System => Self::System,
            Core::Host => Self::Host,
        }
    }
}

impl From<WireTurnInstructionKind> for meerkat_core::lifecycle::run_primitive::TurnInstructionKind {
    fn from(value: WireTurnInstructionKind) -> Self {
        match value {
            WireTurnInstructionKind::User => Self::User,
            WireTurnInstructionKind::System => Self::System,
            WireTurnInstructionKind::Host => Self::Host,
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::TurnInstruction`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTurnInstruction {
    pub kind: WireTurnInstructionKind,
    pub body: String,
}

impl From<meerkat_core::lifecycle::run_primitive::TurnInstruction> for WireTurnInstruction {
    fn from(value: meerkat_core::lifecycle::run_primitive::TurnInstruction) -> Self {
        Self {
            kind: value.kind.into(),
            body: value.body,
        }
    }
}

impl From<WireTurnInstruction> for meerkat_core::lifecycle::run_primitive::TurnInstruction {
    fn from(value: WireTurnInstruction) -> Self {
        Self {
            kind: value.kind.into(),
            body: value.body,
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::ReasoningMode`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireReasoningMode {
    Emit,
    Silent,
    Off,
}

impl From<meerkat_core::lifecycle::run_primitive::ReasoningMode> for WireReasoningMode {
    fn from(value: meerkat_core::lifecycle::run_primitive::ReasoningMode) -> Self {
        use meerkat_core::lifecycle::run_primitive::ReasoningMode as Core;
        match value {
            Core::Emit => Self::Emit,
            Core::Silent => Self::Silent,
            Core::Off => Self::Off,
        }
    }
}

impl From<WireReasoningMode> for meerkat_core::lifecycle::run_primitive::ReasoningMode {
    fn from(value: WireReasoningMode) -> Self {
        match value {
            WireReasoningMode::Emit => Self::Emit,
            WireReasoningMode::Silent => Self::Silent,
            WireReasoningMode::Off => Self::Off,
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::ReasoningEffort`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireReasoningEffort {
    Low,
    Medium,
    High,
}

impl From<meerkat_core::lifecycle::run_primitive::ReasoningEffort> for WireReasoningEffort {
    fn from(value: meerkat_core::lifecycle::run_primitive::ReasoningEffort) -> Self {
        use meerkat_core::lifecycle::run_primitive::ReasoningEffort as Core;
        match value {
            Core::Low => Self::Low,
            Core::Medium => Self::Medium,
            Core::High => Self::High,
        }
    }
}

impl From<WireReasoningEffort> for meerkat_core::lifecycle::run_primitive::ReasoningEffort {
    fn from(value: WireReasoningEffort) -> Self {
        match value {
            WireReasoningEffort::Low => Self::Low,
            WireReasoningEffort::Medium => Self::Medium,
            WireReasoningEffort::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireAnthropicThinkingConfig {
    Adaptive,
    Enabled { budget_tokens: u32 },
}

impl From<meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig>
    for WireAnthropicThinkingConfig
{
    fn from(value: meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig) -> Self {
        use meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig as Core;
        match value {
            Core::Adaptive => Self::Adaptive,
            Core::Enabled { budget_tokens } => Self::Enabled { budget_tokens },
        }
    }
}

impl From<WireAnthropicThinkingConfig>
    for meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig
{
    fn from(value: WireAnthropicThinkingConfig) -> Self {
        match value {
            WireAnthropicThinkingConfig::Adaptive => Self::Adaptive,
            WireAnthropicThinkingConfig::Enabled { budget_tokens } => {
                Self::Enabled { budget_tokens }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireAnthropicEffort {
    Low,
    Medium,
    High,
    Max,
    #[serde(rename = "xhigh")]
    XHigh,
}

impl From<meerkat_core::lifecycle::run_primitive::AnthropicEffort> for WireAnthropicEffort {
    fn from(value: meerkat_core::lifecycle::run_primitive::AnthropicEffort) -> Self {
        use meerkat_core::lifecycle::run_primitive::AnthropicEffort as Core;
        match value {
            Core::Low => Self::Low,
            Core::Medium => Self::Medium,
            Core::High => Self::High,
            Core::Max => Self::Max,
            Core::XHigh => Self::XHigh,
        }
    }
}

impl From<WireAnthropicEffort> for meerkat_core::lifecycle::run_primitive::AnthropicEffort {
    fn from(value: WireAnthropicEffort) -> Self {
        match value {
            WireAnthropicEffort::Low => Self::Low,
            WireAnthropicEffort::Medium => Self::Medium,
            WireAnthropicEffort::High => Self::High,
            WireAnthropicEffort::Max => Self::Max,
            WireAnthropicEffort::XHigh => Self::XHigh,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireAnthropicInferenceGeo {
    Us,
    Global,
    Other { region: String },
}

impl From<meerkat_core::lifecycle::run_primitive::AnthropicInferenceGeo>
    for WireAnthropicInferenceGeo
{
    fn from(value: meerkat_core::lifecycle::run_primitive::AnthropicInferenceGeo) -> Self {
        use meerkat_core::lifecycle::run_primitive::AnthropicInferenceGeo as Core;
        match value {
            Core::Us => Self::Us,
            Core::Global => Self::Global,
            Core::Other { region } => Self::Other { region },
        }
    }
}

impl From<WireAnthropicInferenceGeo>
    for meerkat_core::lifecycle::run_primitive::AnthropicInferenceGeo
{
    fn from(value: WireAnthropicInferenceGeo) -> Self {
        match value {
            WireAnthropicInferenceGeo::Us => Self::Us,
            WireAnthropicInferenceGeo::Global => Self::Global,
            WireAnthropicInferenceGeo::Other { region } => Self::Other { region },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireAnthropicCompactionConfig {
    Auto,
    Custom { edit: serde_json::Value },
}

impl From<meerkat_core::lifecycle::run_primitive::AnthropicCompactionConfig>
    for WireAnthropicCompactionConfig
{
    fn from(value: meerkat_core::lifecycle::run_primitive::AnthropicCompactionConfig) -> Self {
        use meerkat_core::lifecycle::run_primitive::AnthropicCompactionConfig as Core;
        match value {
            Core::Auto => Self::Auto,
            Core::Custom { edit } => Self::Custom {
                edit: edit.as_value(),
            },
        }
    }
}

impl From<WireAnthropicCompactionConfig>
    for meerkat_core::lifecycle::run_primitive::AnthropicCompactionConfig
{
    fn from(value: WireAnthropicCompactionConfig) -> Self {
        use meerkat_core::lifecycle::run_primitive::OpaqueProviderBody;
        match value {
            WireAnthropicCompactionConfig::Auto => Self::Auto,
            WireAnthropicCompactionConfig::Custom { edit } => Self::Custom {
                edit: OpaqueProviderBody::from_value(&edit),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireAnthropicContextWindow {
    OneMegabyte,
}

impl From<meerkat_core::lifecycle::run_primitive::AnthropicContextWindow>
    for WireAnthropicContextWindow
{
    fn from(value: meerkat_core::lifecycle::run_primitive::AnthropicContextWindow) -> Self {
        match value {
            meerkat_core::lifecycle::run_primitive::AnthropicContextWindow::OneMegabyte => {
                Self::OneMegabyte
            }
        }
    }
}

impl From<WireAnthropicContextWindow>
    for meerkat_core::lifecycle::run_primitive::AnthropicContextWindow
{
    fn from(value: WireAnthropicContextWindow) -> Self {
        match value {
            WireAnthropicContextWindow::OneMegabyte => Self::OneMegabyte,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireGeminiThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl From<meerkat_core::lifecycle::run_primitive::GeminiThinkingLevel> for WireGeminiThinkingLevel {
    fn from(value: meerkat_core::lifecycle::run_primitive::GeminiThinkingLevel) -> Self {
        use meerkat_core::lifecycle::run_primitive::GeminiThinkingLevel as Core;
        match value {
            Core::Minimal => Self::Minimal,
            Core::Low => Self::Low,
            Core::Medium => Self::Medium,
            Core::High => Self::High,
        }
    }
}

impl From<WireGeminiThinkingLevel> for meerkat_core::lifecycle::run_primitive::GeminiThinkingLevel {
    fn from(value: WireGeminiThinkingLevel) -> Self {
        match value {
            WireGeminiThinkingLevel::Minimal => Self::Minimal,
            WireGeminiThinkingLevel::Low => Self::Low,
            WireGeminiThinkingLevel::Medium => Self::Medium,
            WireGeminiThinkingLevel::High => Self::High,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireGeminiThinkingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<WireGeminiThinkingLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
}

impl From<meerkat_core::lifecycle::run_primitive::GeminiThinkingConfig>
    for WireGeminiThinkingConfig
{
    fn from(value: meerkat_core::lifecycle::run_primitive::GeminiThinkingConfig) -> Self {
        Self {
            include_thoughts: value.include_thoughts,
            thinking_level: value.thinking_level.map(Into::into),
            thinking_budget: value.thinking_budget,
        }
    }
}

impl From<WireGeminiThinkingConfig>
    for meerkat_core::lifecycle::run_primitive::GeminiThinkingConfig
{
    fn from(value: WireGeminiThinkingConfig) -> Self {
        Self {
            include_thoughts: value.include_thoughts,
            thinking_level: value.thinking_level.map(Into::into),
            thinking_budget: value.thinking_budget,
        }
    }
}

fn deserialize_present_json_value_option<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::ProviderTag`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum WireProviderTag {
    Anthropic {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<WireAnthropicThinkingConfig>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_budget_tokens: Option<u32>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_json_value_option",
            skip_serializing_if = "Option::is_none"
        )]
        web_search: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top_k: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<WireAnthropicEffort>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<meerkat_core::OutputSchema>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inference_geo: Option<WireAnthropicInferenceGeo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compaction: Option<WireAnthropicCompactionConfig>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<WireAnthropicContextWindow>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supports_temperature_override: Option<bool>,
    },
    OpenAi {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<WireReasoningEffort>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seed: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frequency_penalty: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        presence_penalty: Option<f32>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_json_value_option",
            skip_serializing_if = "Option::is_none"
        )]
        web_search: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<meerkat_core::OutputSchema>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_json_value_option",
            skip_serializing_if = "Option::is_none"
        )]
        reasoning: Option<serde_json::Value>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_json_value_option",
            skip_serializing_if = "Option::is_none"
        )]
        chat_template_kwargs: Option<serde_json::Value>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_json_value_option",
            skip_serializing_if = "Option::is_none"
        )]
        thinking: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supports_temperature_override: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supports_reasoning_override: Option<bool>,
    },
    Gemini {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<WireGeminiThinkingConfig>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_budget: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_level: Option<WireGeminiThinkingLevel>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top_k: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        top_p: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<meerkat_core::OutputSchema>,
        #[serde(
            default,
            deserialize_with = "deserialize_present_json_value_option",
            skip_serializing_if = "Option::is_none"
        )]
        google_search: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        candidate_count: Option<u32>,
    },
    /// Wire projection of `ProviderTag::Unknown { bag }` — the typed
    /// escape hatch for V3 legacy-row provider knobs that don't (yet) map
    /// to a typed variant.
    Unknown { bag: StructuredProviderExtension },
}

impl From<meerkat_core::lifecycle::run_primitive::ProviderTag> for WireProviderTag {
    fn from(value: meerkat_core::lifecycle::run_primitive::ProviderTag) -> Self {
        use meerkat_core::lifecycle::run_primitive::ProviderTag as Core;
        match value {
            Core::Anthropic(t) => Self::Anthropic {
                thinking: t.thinking.map(Into::into),
                thinking_budget_tokens: t.thinking_budget_tokens,
                web_search: t.web_search.map(|v| v.as_value()),
                top_k: t.top_k,
                effort: t.effort.map(Into::into),
                structured_output: t.structured_output,
                inference_geo: t.inference_geo.map(Into::into),
                compaction: t.compaction.map(Into::into),
                context: t.context.map(Into::into),
                supports_temperature_override: t.supports_temperature_override,
            },
            Core::OpenAi(t) => Self::OpenAi {
                reasoning_effort: t.reasoning_effort.map(Into::into),
                seed: t.seed,
                frequency_penalty: t.frequency_penalty,
                presence_penalty: t.presence_penalty,
                web_search: t.web_search.map(|v| v.as_value()),
                structured_output: t.structured_output,
                reasoning: t.reasoning.map(|v| v.as_value()),
                chat_template_kwargs: t.chat_template_kwargs.map(|v| v.as_value()),
                thinking: t.thinking.map(|v| v.as_value()),
                supports_temperature_override: t.supports_temperature_override,
                supports_reasoning_override: t.supports_reasoning_override,
            },
            Core::Gemini(t) => Self::Gemini {
                thinking: t.thinking.map(Into::into),
                thinking_budget: t.thinking_budget,
                thinking_level: t.thinking_level.map(Into::into),
                top_k: t.top_k,
                top_p: t.top_p,
                structured_output: t.structured_output,
                google_search: t.google_search.map(|v| v.as_value()),
                candidate_count: t.candidate_count,
            },
            Core::Unknown { bag } => Self::Unknown { bag },
        }
    }
}

impl From<WireProviderTag> for meerkat_core::lifecycle::run_primitive::ProviderTag {
    fn from(value: WireProviderTag) -> Self {
        use meerkat_core::lifecycle::run_primitive::{
            AnthropicProviderTag, GeminiProviderTag, OpaqueProviderBody, OpenAiProviderTag,
        };
        match value {
            WireProviderTag::Anthropic {
                thinking,
                thinking_budget_tokens,
                web_search,
                top_k,
                effort,
                structured_output,
                inference_geo,
                compaction,
                context,
                supports_temperature_override,
            } => Self::Anthropic(AnthropicProviderTag {
                thinking: thinking.map(Into::into),
                thinking_budget_tokens,
                web_search: web_search.map(|v| OpaqueProviderBody::from_value(&v)),
                top_k,
                effort: effort.map(Into::into),
                structured_output,
                inference_geo: inference_geo.map(Into::into),
                compaction: compaction.map(Into::into),
                context: context.map(Into::into),
                supports_temperature_override,
            }),
            WireProviderTag::OpenAi {
                reasoning_effort,
                seed,
                frequency_penalty,
                presence_penalty,
                web_search,
                structured_output,
                reasoning,
                chat_template_kwargs,
                thinking,
                supports_temperature_override,
                supports_reasoning_override,
            } => Self::OpenAi(OpenAiProviderTag {
                reasoning_effort: reasoning_effort.map(Into::into),
                seed,
                frequency_penalty,
                presence_penalty,
                web_search: web_search.map(|v| OpaqueProviderBody::from_value(&v)),
                structured_output,
                reasoning: reasoning.map(|v| OpaqueProviderBody::from_value(&v)),
                chat_template_kwargs: chat_template_kwargs
                    .map(|v| OpaqueProviderBody::from_value(&v)),
                thinking: thinking.map(|v| OpaqueProviderBody::from_value(&v)),
                supports_temperature_override,
                supports_reasoning_override,
            }),
            WireProviderTag::Gemini {
                thinking,
                thinking_budget,
                thinking_level,
                top_k,
                top_p,
                structured_output,
                google_search,
                candidate_count,
            } => Self::Gemini(GeminiProviderTag {
                thinking: thinking.map(Into::into),
                thinking_budget,
                thinking_level: thinking_level.map(Into::into),
                top_k,
                top_p,
                structured_output,
                google_search: google_search.map(|v| OpaqueProviderBody::from_value(&v)),
                candidate_count,
            }),
            WireProviderTag::Unknown { bag } => Self::Unknown { bag },
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::ProviderParamsOverride`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProviderParamsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<WireReasoningMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_tag: Option<WireProviderTag>,
}

impl From<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>
    for WireProviderParamsOverride
{
    fn from(value: meerkat_core::lifecycle::run_primitive::ProviderParamsOverride) -> Self {
        Self {
            temperature: value.temperature,
            top_p: value.top_p,
            max_output_tokens: value.max_output_tokens,
            reasoning: value.reasoning.map(Into::into),
            thinking_budget_tokens: value.thinking_budget_tokens,
            provider_tag: value.provider_tag.map(Into::into),
        }
    }
}

impl From<WireProviderParamsOverride>
    for meerkat_core::lifecycle::run_primitive::ProviderParamsOverride
{
    fn from(value: WireProviderParamsOverride) -> Self {
        Self {
            temperature: value.temperature,
            top_p: value.top_p,
            max_output_tokens: value.max_output_tokens,
            reasoning: value.reasoning.map(Into::into),
            thinking_budget_tokens: value.thinking_budget_tokens,
            provider_tag: value.provider_tag.map(Into::into),
        }
    }
}

/// Wire tri-state override for per-turn runtime metadata fields.
///
/// `None` on the containing field means preserve, `Set` provides a new value,
/// and `Clear` removes the durable value for this turn.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum WireTurnMetadataOverride<T> {
    Set(T),
    Clear,
}

impl<'de, T> Deserialize<'de> for WireTurnMetadataOverride<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(deserializer)?;
        if let Some(object) = raw.as_object() {
            let Some(action_value) = object.get("action") else {
                return serde_json::from_value(raw)
                    .map(Self::Set)
                    .map_err(de::Error::custom);
            };
            let action = action_value.as_str().ok_or_else(|| {
                de::Error::custom("turn metadata override action must be a string")
            })?;
            return match action {
                "clear" => {
                    if object.contains_key("value") {
                        return Err(de::Error::custom("clear override cannot include value"));
                    }
                    Ok(Self::Clear)
                }
                "set" => {
                    let value = object
                        .get("value")
                        .ok_or_else(|| de::Error::custom("set override is missing value"))?;
                    serde_json::from_value(value.clone())
                        .map(Self::Set)
                        .map_err(de::Error::custom)
                }
                other => Err(de::Error::custom(format!(
                    "unknown turn metadata override action `{other}`"
                ))),
            };
        }

        serde_json::from_value(raw)
            .map(Self::Set)
            .map_err(de::Error::custom)
    }
}

impl<T, U> From<meerkat_core::lifecycle::run_primitive::TurnMetadataOverride<T>>
    for WireTurnMetadataOverride<U>
where
    U: From<T>,
{
    fn from(value: meerkat_core::lifecycle::run_primitive::TurnMetadataOverride<T>) -> Self {
        match value {
            meerkat_core::lifecycle::run_primitive::TurnMetadataOverride::Set(value) => {
                Self::Set(value.into())
            }
            meerkat_core::lifecycle::run_primitive::TurnMetadataOverride::Clear => Self::Clear,
        }
    }
}

impl<T, U> From<WireTurnMetadataOverride<T>>
    for meerkat_core::lifecycle::run_primitive::TurnMetadataOverride<U>
where
    U: From<T>,
{
    fn from(value: WireTurnMetadataOverride<T>) -> Self {
        match value {
            WireTurnMetadataOverride::Set(value) => Self::Set(value.into()),
            WireTurnMetadataOverride::Clear => Self::Clear,
        }
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata`].
///
/// The per-turn seam between control plane and core is fully typed —
/// `serde_json::Value` does not appear here.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRuntimeTurnMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling_mode: Option<crate::wire::mob::WireHandlingMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<meerkat_core::TurnToolOverlay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<WireTurnInstruction>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<meerkat_core::Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<WireTurnMetadataOverride<WireProviderParamsOverride>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref:
        Option<WireTurnMetadataOverride<crate::wire::connection::WireConnectionRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<WireKeepAlivePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<crate::wire::mob::WireRenderMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_kind: Option<WireRuntimeExecutionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_response_terminal_apply_intent: Option<WirePeerResponseTerminalApplyIntent>,
}

#[derive(Deserialize)]
struct WireRuntimeTurnMetadataFields {
    #[serde(default)]
    handling_mode: Option<crate::wire::mob::WireHandlingMode>,
    #[serde(default)]
    skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
    #[serde(default)]
    flow_tool_overlay: Option<meerkat_core::TurnToolOverlay>,
    #[serde(default)]
    additional_instructions: Option<Vec<WireTurnInstruction>>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<meerkat_core::Provider>,
    #[serde(default)]
    provider_params: Option<WireTurnMetadataOverride<WireProviderParamsOverride>>,
    #[serde(default)]
    connection_ref: Option<WireTurnMetadataOverride<crate::wire::connection::WireConnectionRef>>,
    #[serde(default)]
    keep_alive: Option<WireKeepAlivePolicy>,
    #[serde(default)]
    render_metadata: Option<crate::wire::mob::WireRenderMetadata>,
    #[serde(default)]
    execution_kind: Option<WireRuntimeExecutionKind>,
    #[serde(default)]
    peer_response_terminal_apply_intent: Option<WirePeerResponseTerminalApplyIntent>,
}

impl<'de> Deserialize<'de> for WireRuntimeTurnMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut raw = serde_json::Value::deserialize(deserializer)?;
        let (clear_provider_params, clear_connection_ref) =
            if let Some(object) = raw.as_object_mut() {
                (
                    take_legacy_clear_bool(object, "clear_provider_params")?,
                    take_legacy_clear_bool(object, "clear_connection_ref")?,
                )
            } else {
                (false, false)
            };
        let fields: WireRuntimeTurnMetadataFields =
            serde_json::from_value(raw).map_err(de::Error::custom)?;
        let provider_params = legacy_wire_override_from_split_fields(
            fields.provider_params,
            clear_provider_params,
            "provider_params",
            "clear_provider_params",
        )?;
        let connection_ref = legacy_wire_override_from_split_fields(
            fields.connection_ref,
            clear_connection_ref,
            "connection_ref",
            "clear_connection_ref",
        )?;

        Ok(Self {
            handling_mode: fields.handling_mode,
            skill_references: fields.skill_references,
            flow_tool_overlay: fields.flow_tool_overlay,
            additional_instructions: fields.additional_instructions,
            model: fields.model,
            provider: fields.provider,
            provider_params,
            connection_ref,
            keep_alive: fields.keep_alive,
            render_metadata: fields.render_metadata,
            execution_kind: fields.execution_kind,
            peer_response_terminal_apply_intent: fields.peer_response_terminal_apply_intent,
        })
    }
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::RuntimeExecutionKind`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRuntimeExecutionKind {
    ContentTurn,
    ResumePending,
}

/// Typed wire projection of
/// [`meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WirePeerResponseTerminalApplyIntent {
    AppendContextAndRun,
}

impl From<meerkat_core::lifecycle::run_primitive::RuntimeExecutionKind>
    for WireRuntimeExecutionKind
{
    fn from(value: meerkat_core::lifecycle::run_primitive::RuntimeExecutionKind) -> Self {
        use meerkat_core::lifecycle::run_primitive::RuntimeExecutionKind as Core;
        match value {
            Core::ContentTurn => Self::ContentTurn,
            Core::ResumePending => Self::ResumePending,
        }
    }
}

impl From<WireRuntimeExecutionKind>
    for meerkat_core::lifecycle::run_primitive::RuntimeExecutionKind
{
    fn from(value: WireRuntimeExecutionKind) -> Self {
        match value {
            WireRuntimeExecutionKind::ContentTurn => Self::ContentTurn,
            WireRuntimeExecutionKind::ResumePending => Self::ResumePending,
        }
    }
}

impl From<meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent>
    for WirePeerResponseTerminalApplyIntent
{
    fn from(
        value: meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent,
    ) -> Self {
        match value {
            meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent::AppendContextAndRun => {
                Self::AppendContextAndRun
            }
        }
    }
}

impl From<WirePeerResponseTerminalApplyIntent>
    for meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent
{
    fn from(value: WirePeerResponseTerminalApplyIntent) -> Self {
        match value {
            WirePeerResponseTerminalApplyIntent::AppendContextAndRun => Self::AppendContextAndRun,
        }
    }
}

impl From<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> for WireRuntimeTurnMetadata {
    fn from(value: meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata) -> Self {
        Self {
            handling_mode: value.handling_mode.map(Into::into),
            skill_references: value.skill_references,
            flow_tool_overlay: value.flow_tool_overlay,
            additional_instructions: value
                .additional_instructions
                .map(|v| v.into_iter().map(Into::into).collect()),
            model: value.model.map(|m| m.as_str().to_string()),
            provider: value.provider,
            provider_params: value.provider_params.map(Into::into),
            connection_ref: value.connection_ref.map(Into::into),
            keep_alive: value.keep_alive.map(Into::into),
            render_metadata: value.render_metadata.map(Into::into),
            execution_kind: value.execution_kind.map(Into::into),
            peer_response_terminal_apply_intent: value
                .peer_response_terminal_apply_intent
                .map(Into::into),
        }
    }
}

impl From<WireRuntimeTurnMetadata> for meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
    fn from(value: WireRuntimeTurnMetadata) -> Self {
        use meerkat_core::lifecycle::run_primitive::ModelId;
        Self {
            handling_mode: value.handling_mode.map(Into::into),
            skill_references: value.skill_references,
            flow_tool_overlay: value.flow_tool_overlay,
            additional_instructions: value
                .additional_instructions
                .map(|v| v.into_iter().map(Into::into).collect()),
            model: value.model.map(ModelId::new),
            provider: value.provider,
            provider_params: value.provider_params.map(Into::into),
            connection_ref: value.connection_ref.map(Into::into),
            keep_alive: value.keep_alive.map(Into::into),
            render_metadata: value.render_metadata.map(Into::into),
            execution_kind: value.execution_kind.map(Into::into),
            peer_response_terminal_apply_intent: value
                .peer_response_terminal_apply_intent
                .map(Into::into),
        }
    }
}

fn legacy_wire_override_from_split_fields<T, E>(
    set_value: Option<WireTurnMetadataOverride<T>>,
    clear: bool,
    set_field: &'static str,
    clear_field: &'static str,
) -> Result<Option<WireTurnMetadataOverride<T>>, E>
where
    E: de::Error,
{
    if clear && set_value.is_some() {
        return Err(E::custom(format!(
            "{clear_field} cannot be combined with {set_field}"
        )));
    }
    if clear {
        Ok(Some(WireTurnMetadataOverride::Clear))
    } else {
        Ok(set_value)
    }
}

fn take_legacy_clear_bool<E>(
    object: &mut serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<bool, E>
where
    E: de::Error,
{
    match object.remove(field) {
        None => Ok(false),
        Some(serde_json::Value::Bool(value)) => Ok(value),
        Some(_) => Err(E::custom(format!("{field} must be a boolean"))),
    }
}
