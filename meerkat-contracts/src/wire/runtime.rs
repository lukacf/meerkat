//! Runtime and input RPC wire contracts.

use serde::{Deserialize, Serialize};

use crate::wire::session::WireContentBlock;
use meerkat_core::comms::PeerName;

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
pub struct SessionPeerResponseTerminalParams {
    pub session_id: String,
    pub peer_name: PeerName,
    pub request_id: String,
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
        peer_name: PeerName,
        request_id: String,
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

/// Discriminator for `session/submit` responses.
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

/// Response payload for `session/submit`.
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

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::ProviderTag`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum WireProviderTag {
    Anthropic {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_budget_tokens: Option<u32>,
    },
    OpenAi {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<WireReasoningEffort>,
    },
    Gemini {
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
                thinking_budget_tokens: t.thinking_budget_tokens,
            },
            Core::OpenAi(t) => Self::OpenAi {
                reasoning_effort: t.reasoning_effort.map(Into::into),
            },
            Core::Gemini(t) => Self::Gemini {
                candidate_count: t.candidate_count,
            },
            Core::Unknown { bag } => Self::Unknown { bag },
        }
    }
}

impl From<WireProviderTag> for meerkat_core::lifecycle::run_primitive::ProviderTag {
    fn from(value: WireProviderTag) -> Self {
        use meerkat_core::lifecycle::run_primitive::{
            AnthropicProviderTag, GeminiProviderTag, OpenAiProviderTag,
        };
        match value {
            WireProviderTag::Anthropic {
                thinking_budget_tokens,
            } => Self::Anthropic(AnthropicProviderTag {
                thinking_budget_tokens,
                ..Default::default()
            }),
            WireProviderTag::OpenAi { reasoning_effort } => Self::OpenAi(OpenAiProviderTag {
                reasoning_effort: reasoning_effort.map(Into::into),
                ..Default::default()
            }),
            WireProviderTag::Gemini { candidate_count } => Self::Gemini(GeminiProviderTag {
                candidate_count,
                ..Default::default()
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

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata`].
///
/// The per-turn seam between control plane and core is fully typed —
/// `serde_json::Value` does not appear here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
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
    pub provider_params: Option<WireProviderParamsOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<crate::wire::connection::WireConnectionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<WireKeepAlivePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<crate::wire::mob::WireRenderMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_kind: Option<WireRuntimeExecutionKind>,
}

/// Typed wire projection of [`meerkat_core::lifecycle::run_primitive::RuntimeExecutionKind`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRuntimeExecutionKind {
    ContentTurn,
    ResumePending,
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
        }
    }
}
