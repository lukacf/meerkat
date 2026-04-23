//! Mob RPC wire contracts.

use super::session::WireContentInput;
use super::supervisor_bridge::BridgeBootstrapToken;
use meerkat_core::OutputSchema;
use meerkat_core::{
    HandlingMode,
    types::{RenderClass, RenderMetadata, RenderSalience},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMobBackendKind {
    #[default]
    Session,
    External,
}

/// Runtime binding for spawn requests.
///
/// First step toward identity-first mobs. Carries backend-specific binding
/// details at spawn time. `External` requires real process identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireRuntimeBinding {
    Session,
    External {
        peer_id: String,
        address: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_token: Option<BridgeBootstrapToken>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMobRuntimeMode {
    #[default]
    AutonomousHost,
    TurnDriven,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobOrchestratorInput {
    pub profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobMcpServerConfigInput {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum MobSkillSourceInput {
    Inline { content: String },
    Path { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobRoleWiringRuleInput {
    pub a: String,
    pub b: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobWiringRulesInput {
    #[serde(default)]
    pub auto_wire_orchestrator: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role_wiring: Vec<MobRoleWiringRuleInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobToolConfigInput {
    #[serde(default)]
    pub builtins: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub comms: bool,
    #[serde(default)]
    pub memory: bool,
    #[serde(default)]
    pub mob: bool,
    #[serde(default)]
    pub mob_tasks: bool,
    #[serde(default)]
    pub schedule: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<String>,
}

/// Profile binding input: either an inline profile or a realm profile reference.
///
/// Not `Eq`: `Inline(MobProfileInput)` transitively carries float provider
/// params (`temperature`, `top_p`) so `Eq` cannot be derived without
/// losing fidelity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum MobProfileBindingInput {
    /// Reference to a realm-scoped profile.
    RealmRef {
        /// Name of the realm profile.
        realm_profile: String,
    },
    /// Inline profile definition.
    Inline(MobProfileInput),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobProfileInput {
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: MobToolConfigInput,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_description: String,
    #[serde(default)]
    pub external_addressable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
    #[serde(default)]
    pub runtime_mode: WireMobRuntimeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_peer_notifications: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    /// Non-`Eq` field: `WireProviderParamsOverride` contains float scalars
    /// (`temperature`, `top_p`) so the struct can't derive `Eq` without
    /// losing fidelity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::wire::runtime::WireProviderParamsOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobExternalBackendConfigInput {
    pub address_base: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobBackendConfigInput {
    #[serde(default)]
    pub default: WireMobBackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<MobExternalBackendConfigInput>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobDispatchModeInput {
    #[default]
    FanOut,
    OneToOne,
    FanIn,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobCollectionPolicyInput {
    #[default]
    All,
    Any,
    Quorum {
        n: u8,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobDependencyModeInput {
    #[default]
    All,
    Any,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobStepOutputFormatInput {
    #[default]
    Json,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MobConditionExprInput {
    Eq { path: String, value: Value },
    In { path: String, values: Vec<Value> },
    Gt { path: String, value: Value },
    Lt { path: String, value: Value },
    And { exprs: Vec<MobConditionExprInput> },
    Or { exprs: Vec<MobConditionExprInput> },
    Not { expr: Box<MobConditionExprInput> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFrameSpecInput {
    pub nodes: BTreeMap<String, MobFlowNodeInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MobFlowNodeInput {
    Step(MobFrameStepInput),
    RepeatUntil(MobRepeatUntilInput),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFrameStepInput {
    pub step_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub depends_on_mode: MobDependencyModeInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobRepeatUntilInput {
    pub loop_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub depends_on_mode: MobDependencyModeInput,
    pub body: MobFrameSpecInput,
    pub until: MobConditionExprInput,
    pub max_iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFlowStepInput {
    pub role: String,
    pub message: WireContentInput,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub dispatch_mode: MobDispatchModeInput,
    #[serde(default)]
    pub collection_policy: MobCollectionPolicyInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<MobConditionExprInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_schema_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub depends_on_mode: MobDependencyModeInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_tools: Option<Vec<String>>,
    #[serde(default)]
    pub output_format: MobStepOutputFormatInput,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFlowSpecInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub steps: BTreeMap<String, MobFlowStepInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<MobFrameSpecInput>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobPolicyModeInput {
    #[default]
    Advisory,
    Strict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobTopologyRuleInput {
    pub from_role: String,
    pub to_role: String,
    pub allowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobTopologySpecInput {
    pub mode: MobPolicyModeInput,
    pub rules: Vec<MobTopologyRuleInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSupervisorSpecInput {
    pub role: String,
    pub escalation_threshold: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobLimitsSpecInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_flow_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_step_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_orphaned_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_grace_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_nodes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frame_depth: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum MobSpawnPolicyInput {
    None,
    Auto {
        profile_map: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobEventRouterConfigInput {
    #[serde(default = "default_event_router_buffer_size")]
    pub buffer_size: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_patterns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_patterns: Option<Vec<String>>,
}

const fn default_event_router_buffer_size() -> usize {
    256
}

/// Public mob definition input for `mob/create`.
///
/// This mirrors the public creation contract shape. Runtime-owned lifecycle and
/// bookkeeping fields such as internal owner/runtime bindings,
/// `session_cleanup_policy`, `is_implicit`, and internal-only profile tool
/// bundles are intentionally not part of this schema.
///
/// Not `Eq`: `profiles` transitively carries float provider params.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobDefinitionInput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<MobOrchestratorInput>,
    pub profiles: BTreeMap<String, MobProfileBindingInput>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_servers: BTreeMap<String, MobMcpServerConfigInput>,
    #[serde(default)]
    pub wiring: MobWiringRulesInput,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<String, MobSkillSourceInput>,
    #[serde(default)]
    pub backend: MobBackendConfigInput,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flows: BTreeMap<String, MobFlowSpecInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<MobTopologySpecInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor: Option<MobSupervisorSpecInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<MobLimitsSpecInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_policy: Option<MobSpawnPolicyInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_router: Option<MobEventRouterConfigInput>,
}

/// Request payload for `mob/create`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobCreateParams {
    pub definition: MobDefinitionInput,
}

/// Response payload for `mob/create`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobCreateResult {
    pub mob_id: String,
}

/// Minimal trusted peer spec for public mob wiring surfaces.
///
/// `pubkey` is the Ed25519 signing public key (32 bytes) required so the
/// receiver can verify envelope signatures after trust registration.
/// Serialized as a 32-element JSON array of numbers (matching
/// `BridgePeerSpec`). Defaults to a zero pubkey for legacy clients —
/// the corresponding `TrustedPeerDescriptor::pubkey` will then be all
/// zeros, which makes signature verification fail closed. Production
/// clients MUST send the real pubkey.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTrustedPeerSpec {
    pub name: String,
    pub peer_id: String,
    pub address: String,
    #[serde(default)]
    pub pubkey: [u8; 32],
}

/// Target for a mob wire/unwire call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobPeerTarget {
    Local(String),
    External(WireTrustedPeerSpec),
}

/// Request payload for `mob/wire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobWireParams {
    pub mob_id: String,
    pub member: String,
    pub peer: MobPeerTarget,
}

/// Response payload for `mob/wire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobWireResult {
    pub wired: bool,
}

/// Request payload for `mob/unwire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobUnwireParams {
    pub mob_id: String,
    pub member: String,
    pub peer: MobPeerTarget,
}

/// Response payload for `mob/unwire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobUnwireResult {
    pub unwired: bool,
}

/// Request payload for host-side mob member delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobMemberSendParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub content: WireContentInput,
    #[serde(default)]
    pub handling_mode: WireHandlingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<WireRenderMetadata>,
}

/// Response payload for host-side mob member delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAgentRuntimeId {
    pub identity: String,
    pub generation: u64,
}

/// Response payload for host-side mob member delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobMemberSendResult {
    pub mob_id: String,
    /// Identity-native member identity (0.6).
    pub agent_identity: String,
    /// Server-resolved opaque handle for subsequent member-targeted calls.
    /// App code routes through `member_ref`; the binding-era
    /// `{identity, generation}` pair carried by `WireAgentRuntimeId` is
    /// retired from app-facing responses per dogma #10.
    pub member_ref: WireMemberRef,
    pub handling_mode: WireHandlingMode,
}

/// Public handling mode for mob member delivery.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireHandlingMode {
    #[default]
    Queue,
    Steer,
}

impl From<WireHandlingMode> for HandlingMode {
    fn from(mode: WireHandlingMode) -> Self {
        match mode {
            WireHandlingMode::Queue => HandlingMode::Queue,
            WireHandlingMode::Steer => HandlingMode::Steer,
        }
    }
}

impl From<HandlingMode> for WireHandlingMode {
    fn from(mode: HandlingMode) -> Self {
        match mode {
            HandlingMode::Queue => WireHandlingMode::Queue,
            HandlingMode::Steer => WireHandlingMode::Steer,
        }
    }
}

/// Public render class contract for mob member delivery.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRenderClass {
    UserPrompt,
    PeerMessage,
    PeerRequest,
    PeerResponse,
    ExternalEvent,
    FlowStep,
    Continuation,
    SystemNotice,
    ToolScopeNotice,
    OpsProgress,
}

impl From<WireRenderClass> for RenderClass {
    fn from(class: WireRenderClass) -> Self {
        match class {
            WireRenderClass::UserPrompt => RenderClass::UserPrompt,
            WireRenderClass::PeerMessage => RenderClass::PeerMessage,
            WireRenderClass::PeerRequest => RenderClass::PeerRequest,
            WireRenderClass::PeerResponse => RenderClass::PeerResponse,
            WireRenderClass::ExternalEvent => RenderClass::ExternalEvent,
            WireRenderClass::FlowStep => RenderClass::FlowStep,
            WireRenderClass::Continuation => RenderClass::Continuation,
            WireRenderClass::SystemNotice => RenderClass::SystemNotice,
            WireRenderClass::ToolScopeNotice => RenderClass::ToolScopeNotice,
            WireRenderClass::OpsProgress => RenderClass::OpsProgress,
        }
    }
}

impl From<RenderClass> for WireRenderClass {
    fn from(class: RenderClass) -> Self {
        match class {
            RenderClass::UserPrompt => WireRenderClass::UserPrompt,
            RenderClass::PeerMessage => WireRenderClass::PeerMessage,
            RenderClass::PeerRequest => WireRenderClass::PeerRequest,
            RenderClass::PeerResponse => WireRenderClass::PeerResponse,
            RenderClass::ExternalEvent => WireRenderClass::ExternalEvent,
            RenderClass::FlowStep => WireRenderClass::FlowStep,
            RenderClass::Continuation => WireRenderClass::Continuation,
            RenderClass::SystemNotice => WireRenderClass::SystemNotice,
            RenderClass::ToolScopeNotice => WireRenderClass::ToolScopeNotice,
            RenderClass::OpsProgress => WireRenderClass::OpsProgress,
        }
    }
}

/// Public render salience contract for mob member delivery.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRenderSalience {
    Background,
    Normal,
    Important,
    Urgent,
}

impl From<WireRenderSalience> for RenderSalience {
    fn from(salience: WireRenderSalience) -> Self {
        match salience {
            WireRenderSalience::Background => RenderSalience::Background,
            WireRenderSalience::Normal => RenderSalience::Normal,
            WireRenderSalience::Important => RenderSalience::Important,
            WireRenderSalience::Urgent => RenderSalience::Urgent,
        }
    }
}

impl From<RenderSalience> for WireRenderSalience {
    fn from(salience: RenderSalience) -> Self {
        match salience {
            RenderSalience::Background => WireRenderSalience::Background,
            RenderSalience::Normal => WireRenderSalience::Normal,
            RenderSalience::Important => WireRenderSalience::Important,
            RenderSalience::Urgent => WireRenderSalience::Urgent,
        }
    }
}

/// Public render metadata contract for mob member delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRenderMetadata {
    pub class: WireRenderClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salience: Option<WireRenderSalience>,
}

impl From<WireRenderMetadata> for RenderMetadata {
    fn from(metadata: WireRenderMetadata) -> Self {
        Self {
            class: metadata.class.into(),
            salience: metadata
                .salience
                .unwrap_or(WireRenderSalience::Normal)
                .into(),
        }
    }
}

impl From<RenderMetadata> for WireRenderMetadata {
    fn from(metadata: RenderMetadata) -> Self {
        Self {
            class: metadata.class.into(),
            salience: Some(metadata.salience.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Declarative roster API (`mob/ensure_member`, `mob/reconcile`,
// `mob/list_members_matching`). These methods compose over spawn / retire /
// list_members; they introduce no new lifecycle.
// ---------------------------------------------------------------------------

/// Per-member spec for `mob/ensure_member` and the `desired` entries of
/// `mob/reconcile`.
///
/// Mirrors the essential, codegen-friendly fields of
/// [`meerkat_mob::SpawnMemberSpec`]. Complex sub-types (tool access policy,
/// budget split, inherited tool filter, override profile) are not on this
/// wire surface — callers that need that parity should use the non-declarative
/// `mob/spawn` method.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobMemberSpecWire {
    /// Profile name (role) in the mob definition.
    pub profile: String,
    /// Stable member identity within the mob.
    pub agent_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<WireContentInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<WireRuntimeBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_wire_parent: Option<bool>,
}

/// Request payload for `mob/ensure_member`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobEnsureMemberParams {
    pub mob_id: String,
    pub spec: MobMemberSpecWire,
}

/// Server-resolved opaque handle for a mob member.
///
/// Encodes `{mob_id, agent_identity}` as a single base64url-encoded token
/// that callers treat as opaque. The server resolves the current
/// `AgentRuntimeId` and fence token against the live mob roster on every
/// dispatch — clients never reason about `generation` or `fence_token`
/// directly.
///
/// Use [`WireMemberRef::encode`] to produce a token and
/// [`WireMemberRef::decode`] inside an RPC handler to recover the
/// `(mob_id, agent_identity)` pair before resolving against the runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct WireMemberRef(String);

impl WireMemberRef {
    /// Construct a handle from its components. The `mob_id` and
    /// `agent_identity` together form the resolution key the server uses to
    /// look up the member's current incarnation.
    #[must_use]
    pub fn encode(mob_id: &str, agent_identity: &str) -> Self {
        // Single-letter keys keep the encoded payload short so the token
        // remains compact in URLs and JSON payloads.
        // `Value::to_string` on a two-field object is infallible.
        let payload = serde_json::json!({ "m": mob_id, "a": agent_identity });
        Self(base64_url_encode(payload.to_string().as_bytes()))
    }

    /// Borrow the raw token string for transport.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Construct a handle from a raw token string without validation. Used
    /// when forwarding an opaque token received from the wire.
    #[must_use]
    pub fn from_token(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// Decode the handle into `(mob_id, agent_identity)`. Returns `Err` when
    /// the token is malformed.
    pub fn decode(&self) -> Result<(String, String), WireMemberRefError> {
        let bytes = base64_url_decode(&self.0).map_err(|_| WireMemberRefError::Malformed)?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| WireMemberRefError::Malformed)?;
        let mob_id = value
            .get("m")
            .and_then(Value::as_str)
            .ok_or(WireMemberRefError::Malformed)?;
        let agent_identity = value
            .get("a")
            .and_then(Value::as_str)
            .ok_or(WireMemberRefError::Malformed)?;
        Ok((mob_id.to_string(), agent_identity.to_string()))
    }
}

/// Failure modes for [`WireMemberRef::decode`].
#[derive(Debug, thiserror::Error)]
pub enum WireMemberRefError {
    /// Token is not valid base64url or its decoded payload is not the
    /// expected `{m, a}` shape.
    #[error("malformed member ref token")]
    Malformed,
}

fn base64_url_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn base64_url_decode(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(input)
}

/// Identity-native payload for `EnsureMemberOutcome::Spawned`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobSpawnReceiptWire {
    pub agent_identity: String,
    /// Server-resolved opaque handle for subsequent member-targeted calls
    /// (work submission, cancellation, lifecycle). Replaces the binding-era
    /// `generation` / `fence_token` pair on app-facing surfaces.
    pub member_ref: WireMemberRef,
}

/// Execution status mirroring `meerkat_mob::runtime::MobMemberStatus`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMobMemberStatus {
    Active,
    Retiring,
    Broken,
    Completed,
    Unknown,
}

/// Public roster entry returned by `mob/ensure_member`'s `Existed` outcome
/// (and other surfaces that want a typed snapshot of a single member). Mirrors
/// the public-facing fields of `meerkat_mob::runtime::MobMemberListEntry`
/// without leaking bridge-internal fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobMemberListEntryWire {
    pub agent_identity: String,
    pub member_ref: WireMemberRef,
    pub role: String,
    pub runtime_mode: WireMobRuntimeMode,
    pub state: WireMemberState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wired_to: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub status: WireMobMemberStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub is_final: bool,
}

/// Outcome of a `mob/ensure_member` call.
///
/// `Existed` returns the typed [`MobMemberListEntryWire`] roster snapshot so
/// public consumers do not need out-of-band knowledge of the Rust domain
/// `MobMemberListEntry` shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MobEnsureMemberOutcomeWire {
    #[serde(rename = "spawned")]
    Spawned(MobSpawnReceiptWire),
    #[serde(rename = "existed")]
    Existed(MobMemberListEntryWire),
}

/// Response payload for `mob/ensure_member`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobEnsureMemberResult {
    pub outcome: MobEnsureMemberOutcomeWire,
}

/// Options controlling a `mob/reconcile` pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobReconcileOptionsWire {
    /// When `true`, members on the roster whose identity is not in the
    /// `desired` set are retired.
    #[serde(default)]
    pub retire_stale: bool,
}

/// Request payload for `mob/reconcile`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobReconcileParams {
    pub mob_id: String,
    #[serde(default)]
    pub desired: Vec<MobMemberSpecWire>,
    #[serde(default)]
    pub options: MobReconcileOptionsWire,
}

/// Per-identity failure in a `mob/reconcile` pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobReconcileFailureWire {
    pub agent_identity: String,
    /// One of `"spawn"` or `"retire"`.
    pub stage: String,
    /// Stringified mob error.
    pub error: String,
}

/// Summary produced by a `mob/reconcile` pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobReconcileReportWire {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desired: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawned: Vec<MobSpawnReceiptWire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<MobReconcileFailureWire>,
}

/// Response payload for `mob/reconcile`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobReconcileResult {
    pub report: MobReconcileReportWire,
}

/// Typed lifecycle action for `mob/lifecycle`. Replaces the prior
/// `action: String` discriminator with an exhaustive enum so callers and
/// handlers reason about lifecycle transitions through the type system
/// rather than string folklore.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMobLifecycleAction {
    Stop,
    Resume,
    Complete,
    Reset,
    Destroy,
}

/// Request payload for `mob/lifecycle`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobLifecycleParams {
    pub mob_id: String,
    pub action: WireMobLifecycleAction,
}

/// Origin for `MobSubmitWorkParams`. Replaces the prior free-form
/// `origin: Option<String>` shape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireWorkOrigin {
    #[default]
    External,
    Internal,
}

/// Request payload for `mob/submit_work`.
///
/// Identifies the member through the opaque [`WireMemberRef`] handle the
/// server resolves against the live roster — callers do not pass
/// `generation` or `fence_token`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSubmitWorkParams {
    pub member_ref: WireMemberRef,
    /// Optional caller-supplied work reference. When absent the server
    /// generates a fresh UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_ref: Option<String>,
    pub content: WireContentInput,
    #[serde(default)]
    pub origin: WireWorkOrigin,
}

/// Response payload for `mob/submit_work`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobSubmitWorkResult {
    pub mob_id: String,
    pub work_ref: String,
    pub member_ref: WireMemberRef,
}

/// Request payload for `mob/cancel_work`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobCancelWorkParams {
    pub mob_id: String,
    pub work_ref: String,
}

/// Request payload for `mob/cancel_all_work`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobCancelAllWorkParams {
    pub member_ref: WireMemberRef,
}

/// Roster member lifecycle state for `MobMemberFilterWire`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMemberState {
    Active,
    Retiring,
}

/// Filter for `mob/list_members_matching`. Non-empty / `Some` fields are
/// combined conjunctively; an empty filter matches every member.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobMemberFilterWire {
    /// Required exact matches on member labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Required profile name (role).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Required roster state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<WireMemberState>,
}

/// Request payload for `mob/list_members_matching`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobListMembersMatchingParams {
    pub mob_id: String,
    #[serde(default)]
    pub filter: MobMemberFilterWire,
}

/// Response payload for `mob/list_members_matching`. Each member is the raw
/// roster entry JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobListMembersMatchingResult {
    #[serde(default)]
    pub members: Vec<Value>,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn wire_member_ref_round_trips_through_encode_decode() {
        let token = WireMemberRef::encode("mob-42", "worker-1");
        let (mob_id, agent_identity) = token.decode().expect("decode round-trips");
        assert_eq!(mob_id, "mob-42");
        assert_eq!(agent_identity, "worker-1");
    }

    #[test]
    fn wire_member_ref_rejects_malformed_token() {
        let err = WireMemberRef::from_token("not-a-token-payload")
            .decode()
            .expect_err("malformed tokens must fail to decode");
        assert!(matches!(err, WireMemberRefError::Malformed));
    }

    #[test]
    fn mob_wire_params_reject_legacy_local_target_shape() {
        let err = serde_json::from_value::<MobWireParams>(serde_json::json!({
            "mob_id": "mob-1",
            "local": "member-a",
            "target": { "local": "member-b" }
        }))
        .expect_err("legacy local/target shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("unknown field `local`") || msg.contains("missing field `member`"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn mob_create_params_reject_reserved_runtime_lifecycle_fields() {
        let err = serde_json::from_value::<MobCreateParams>(serde_json::json!({
            "definition": {
                "id": "mob-1",
                "owner_runtime_binding": "runtime:worker:0",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        }))
        .expect_err("reserved runtime lifecycle fields must be rejected");

        assert!(
            err.to_string()
                .contains("unknown field `owner_runtime_binding`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_create_params_reject_reserved_runtime_bridge_owner_field() {
        let err = serde_json::from_value::<MobCreateParams>(serde_json::json!({
            "definition": {
                "id": "mob-1",
                "owner_transport_binding": "transport:worker:0",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        }))
        .expect_err("reserved runtime bridge owner field must be rejected");

        assert!(
            err.to_string()
                .contains("unknown field `owner_transport_binding`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_create_params_reject_internal_profile_tool_bundles() {
        let err = serde_json::from_value::<MobCreateParams>(serde_json::json!({
            "definition": {
                "id": "mob-1",
                "profiles": {
                    "worker": {
                        "model": "claude-sonnet-4-6",
                        "tools": {
                            "rust_bundles": ["internal-only"]
                        }
                    }
                }
            }
        }))
        .expect_err("internal rust tool bundles must be rejected");

        // With untagged MobProfileBindingInput, the error message is about
        // no variant matching rather than the specific unknown field.
        assert!(
            err.to_string().contains("did not match any variant")
                || err.to_string().contains("unknown field `rust_bundles`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_create_params_accept_typed_nested_flow_definition() {
        let params = serde_json::from_value::<MobCreateParams>(serde_json::json!({
            "definition": {
                "id": "mob-1",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                },
                "flows": {
                    "review": {
                        "description": "review flow",
                        "steps": {
                            "draft": {
                                "role": "worker",
                                "message": "draft it"
                            }
                        }
                    }
                }
            }
        }))
        .expect("typed nested flow definition should parse");

        assert_eq!(
            params.definition.flows["review"].steps["draft"].role,
            "worker"
        );
    }
}
