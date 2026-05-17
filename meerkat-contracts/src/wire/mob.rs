//! Mob RPC wire contracts.

use super::connection::WireAuthBindingRef;
use super::session::WireContentInput;
use super::supervisor_bridge::BridgeBootstrapToken;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use meerkat_core::OutputSchema;
use meerkat_core::{
    HandlingMode,
    types::{RenderClass, RenderMetadata, RenderSalience},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use meerkat_core::{SurfaceMetadata, SurfaceMetadataError};

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
/// details at spawn time. `External` requires typed process identity; callers
/// do not supply raw comms peer IDs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireRuntimeBinding {
    Session,
    External {
        address: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_token: Option<BridgeBootstrapToken>,
        /// Typed Ed25519 signing identity for the external process. The
        /// canonical comms `PeerId` is derived from this key after the wire
        /// boundary, so callers cannot spoof an unrelated raw peer id.
        identity: WireTrustedPeerIdentity,
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

/// How a mob member should be launched by `mob/spawn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WireMemberLaunchMode {
    Fresh,
    Resume {
        #[serde(alias = "session_id")]
        bridge_session_id: String,
    },
    Fork {
        source_member_id: String,
        #[serde(default)]
        fork_context: WireForkContext,
    },
}

/// Conversation history scope used when forking a mob member.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireForkContext {
    #[default]
    FullHistory,
    LastMessages {
        count: u32,
    },
}

/// Budget split policy for a spawned mob member.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum WireBudgetSplitPolicy {
    #[default]
    Equal,
    Proportional,
    Remaining,
    Fixed(u64),
}

/// Tool access policy for a spawned mob member.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum WireToolAccessPolicy {
    #[default]
    Inherit,
    AllowList(Vec<String>),
    DenyList(Vec<String>),
}

/// Pre-resolved tool filter inherited by a spawned mob member.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireToolFilter {
    #[default]
    All,
    Allow(Vec<String>),
    Deny(Vec<String>),
}

/// Tool configuration embedded in a wire mob profile override.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireMobToolConfig {
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
    pub schedule: bool,
    #[serde(default)]
    pub image_generation: bool,
    #[serde(default)]
    pub mcp: Vec<String>,
}

/// Profile override for `mob/spawn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireMobProfile {
    pub model: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: WireMobToolConfig,
    #[serde(default)]
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
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobOrchestratorInput {
    pub profile: String,
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
    pub schedule: bool,
    #[serde(default)]
    pub image_generation: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<String>,
}

/// Profile binding input: either an inline profile or a realm profile reference.
///
/// Not `Eq`: `Inline(MobProfileInput)` transitively carries float provider
/// params (`temperature`, `top_p`) so `Eq` cannot be derived without
/// losing fidelity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(clippy::large_enum_variant)]
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

/// Shared request payload for mob methods that address a mob by id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobIdParams {
    pub mob_id: String,
}

/// Shared request payload for mob methods that address one member by identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobMemberParams {
    pub mob_id: String,
    pub agent_identity: String,
}

/// One active mob row returned by `mob/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobStatusResult {
    pub mob_id: String,
    pub status: String,
}

/// Response payload for `mob/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobListResult {
    pub mobs: Vec<MobStatusResult>,
}

/// Request payload for `mob/spawn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSpawnParams {
    pub mob_id: String,
    pub profile: String,
    pub agent_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<WireContentInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<WireRuntimeBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_wire_parent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_mode: Option<WireMemberLaunchMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<WireToolAccessPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_split_policy: Option<WireBudgetSplitPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_tool_filter: Option<WireToolFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_profile: Option<WireMobProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<WireAuthBindingRef>,
}

/// Response payload for `mob/spawn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobSpawnResult {
    pub mob_id: String,
    pub agent_identity: String,
    pub member_ref: WireMemberRef,
}

/// Per-member request payload inside `mob/spawn_many`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSpawnSpecParams {
    pub profile: String,
    pub agent_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<WireContentInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<WireAuthBindingRef>,
}

/// Request payload for `mob/spawn_many`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSpawnManyParams {
    pub mob_id: String,
    pub specs: Vec<MobSpawnSpecParams>,
}

/// Typed status for one `mob/spawn_many` row.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobSpawnManyResultStatus {
    Spawned,
    Failed,
}

/// Successful per-member `mob/spawn_many` result payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSpawnManySpawnedResult {
    pub agent_identity: String,
    pub member_ref: WireMemberRef,
}

/// Typed failure cause for one failed `mob/spawn_many` member row.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobSpawnManyFailureCause {
    ProfileNotFound,
    MemberNotFound,
    MemberAlreadyExists,
    NotExternallyAddressable,
    InvalidTransition,
    WiringError,
    BridgeCommandRejected,
    MemberRestoreFailed,
    KickoffWaitTimedOut,
    ReadyWaitTimedOut,
    DefinitionError,
    FlowNotFound,
    FlowFailed,
    RunNotFound,
    RunCanceled,
    FlowTurnTimedOut,
    FrameDepthLimitExceeded,
    FrameAtomicPersistenceUnavailable,
    SpecRevisionConflict,
    SchemaValidation,
    InsufficientTargets,
    TopologyViolation,
    BridgeDeliveryRejected,
    SupervisorEscalation,
    UnsupportedForMode,
    MissingMemberCapability,
    ResetBarrier,
    StorageError,
    SessionError,
    CommsError,
    CallbackPending,
    StaleFenceToken,
    StaleEventCursor,
    WorkNotFound,
    Internal,
}

/// Failed per-member `mob/spawn_many` result payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSpawnManyFailedResult {
    pub cause: MobSpawnManyFailureCause,
    pub message: String,
}

/// Typed payload for one `mob/spawn_many` row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum MobSpawnManyResultPayload {
    Spawned(MobSpawnManySpawnedResult),
    Failed(MobSpawnManyFailedResult),
}

/// One typed result entry in a `mob/spawn_many` response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "MobSpawnManyResultEntryRaw")]
pub struct MobSpawnManyResultEntry {
    pub status: MobSpawnManyResultStatus,
    pub result: MobSpawnManyResultPayload,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
struct MobSpawnManyResultEntryRaw {
    status: MobSpawnManyResultStatus,
    result: MobSpawnManyResultPayload,
}

impl TryFrom<MobSpawnManyResultEntryRaw> for MobSpawnManyResultEntry {
    type Error = String;

    fn try_from(raw: MobSpawnManyResultEntryRaw) -> Result<Self, Self::Error> {
        let entry = Self {
            status: raw.status,
            result: raw.result,
        };
        entry.validate().map_err(str::to_owned)?;
        Ok(entry)
    }
}

impl MobSpawnManyResultEntry {
    pub fn spawned(agent_identity: impl Into<String>, member_ref: WireMemberRef) -> Self {
        Self {
            status: MobSpawnManyResultStatus::Spawned,
            result: MobSpawnManyResultPayload::Spawned(MobSpawnManySpawnedResult {
                agent_identity: agent_identity.into(),
                member_ref,
            }),
        }
    }

    pub fn failed(cause: MobSpawnManyFailureCause, message: impl Into<String>) -> Self {
        Self {
            status: MobSpawnManyResultStatus::Failed,
            result: MobSpawnManyResultPayload::Failed(MobSpawnManyFailedResult {
                cause,
                message: message.into(),
            }),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match (&self.status, &self.result) {
            (MobSpawnManyResultStatus::Spawned, MobSpawnManyResultPayload::Spawned(_))
            | (MobSpawnManyResultStatus::Failed, MobSpawnManyResultPayload::Failed(_)) => Ok(()),
            (MobSpawnManyResultStatus::Spawned, MobSpawnManyResultPayload::Failed(_)) => {
                Err("mob spawn_many result status spawned requires spawned result")
            }
            (MobSpawnManyResultStatus::Failed, MobSpawnManyResultPayload::Spawned(_)) => {
                Err("mob spawn_many result status failed requires failed result")
            }
        }
    }
}

/// Response payload for `mob/spawn_many`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobSpawnManyResult {
    pub results: Vec<MobSpawnManyResultEntry>,
}

/// Response payload for `mob/retire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobRetireResult {
    pub retired: bool,
}

/// Request payload for `mob/respawn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobRespawnParams {
    pub mob_id: String,
    pub agent_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_message: Option<WireContentInput>,
}

/// Identity-native respawn receipt returned inside `MobRespawnResult`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobRespawnReceipt {
    pub identity: String,
    pub member_ref: WireMemberRef,
}

/// Response payload for `mob/respawn`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobRespawnResult {
    pub status: String,
    pub receipt: MobRespawnReceipt,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_peer_ids: Vec<String>,
}

/// Response payload for `mob/members`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobMembersResult {
    pub mob_id: String,
    pub members: Vec<MobMemberListEntryWire>,
}

/// Request payload for `mob/events`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobEventsParams {
    pub mob_id: String,
    #[serde(default)]
    pub after_cursor: u64,
    #[serde(default = "default_mob_events_limit")]
    pub limit: usize,
    #[serde(default)]
    pub strict: bool,
}

const fn default_mob_events_limit() -> usize {
    100
}

/// Response payload for `mob/events`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobEventsResult {
    pub events: Vec<Value>,
}

/// Typed external peer identity for public mob wiring surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireTrustedPeerIdentity {
    /// Recoverable Ed25519 public key string in `ed25519:<base64>` form.
    Ed25519PublicKey { public_key: String },
}

/// Resolved external peer identity atoms used after the wire boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedWireTrustedPeerIdentity {
    pub peer_id: meerkat_core::comms::PeerId,
    pub pubkey: [u8; 32],
}

/// Failure modes for resolving a typed external peer identity.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum WireTrustedPeerIdentityError {
    #[error("external peer identity public_key must start with 'ed25519:'")]
    MissingEd25519Prefix,
    #[error("external peer identity public_key is not valid base64: {0}")]
    InvalidBase64(String),
    #[error("external peer identity public_key must decode to 32 bytes, got {actual}")]
    InvalidLength { actual: usize },
    #[error("external peer identity public_key must be non-zero")]
    ZeroPublicKey,
}

impl WireTrustedPeerIdentity {
    pub fn resolve(&self) -> Result<ResolvedWireTrustedPeerIdentity, WireTrustedPeerIdentityError> {
        match self {
            Self::Ed25519PublicKey { public_key } => {
                let pubkey = parse_ed25519_public_key(public_key)?;
                if pubkey == [0u8; 32] {
                    return Err(WireTrustedPeerIdentityError::ZeroPublicKey);
                }
                Ok(ResolvedWireTrustedPeerIdentity {
                    peer_id: meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey),
                    pubkey,
                })
            }
        }
    }
}

fn parse_ed25519_public_key(raw: &str) -> Result<[u8; 32], WireTrustedPeerIdentityError> {
    const PREFIX: &str = "ed25519:";
    let encoded = raw
        .strip_prefix(PREFIX)
        .ok_or(WireTrustedPeerIdentityError::MissingEd25519Prefix)?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|err| WireTrustedPeerIdentityError::InvalidBase64(err.to_string()))?;
    let actual = bytes.len();
    let pubkey: [u8; 32] = bytes
        .try_into()
        .map_err(|_| WireTrustedPeerIdentityError::InvalidLength { actual })?;
    Ok(pubkey)
}

/// Minimal trusted peer spec for public mob wiring surfaces.
///
/// `identity` is required and resolves to the Ed25519 signing public key
/// plus the canonical comms `PeerId` derived from that key. MCP callers do
/// not provide raw peer IDs, and missing key material fails at the boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTrustedPeerSpec {
    pub name: String,
    pub address: String,
    pub identity: WireTrustedPeerIdentity,
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

/// One local-member edge in `mob/wire_members_batch`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobWireMembersBatchEdge {
    pub a: String,
    pub b: String,
}

/// Request payload for `mob/wire_members_batch`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobWireMembersBatchParams {
    pub mob_id: String,
    pub edges: Vec<MobWireMembersBatchEdge>,
}

/// Response payload for `mob/wire_members_batch`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobWireMembersBatchResult {
    pub requested: usize,
    pub wired: Vec<MobWireMembersBatchEdge>,
    pub already_wired: Vec<MobWireMembersBatchEdge>,
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

/// Request payload for `mob/ingress_interaction`.
///
/// This is the ergonomic "ensure an ingress member, then deliver user input"
/// path. It composes the existing declarative roster and member-send
/// semantics without introducing a separate thread/project runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobIngressInteractionParams {
    pub mob_id: String,
    pub spec: MobMemberSpecWire,
    pub content: WireContentInput,
    #[serde(default)]
    pub handling_mode: WireHandlingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<WireRenderMetadata>,
}

/// Response payload for `mob/ingress_interaction`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobIngressInteractionResult {
    pub mob_id: String,
    pub agent_identity: String,
    pub member_ref: WireMemberRef,
    pub ensure_outcome: MobEnsureMemberOutcomeWire,
    pub delivery: MobMemberSendResult,
    /// Cursor observed immediately before the ensure/send composition.
    pub events_after_cursor: u64,
    /// Cursor observed after delivery was accepted.
    pub latest_event_cursor: u64,
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

impl MobMemberSpecWire {
    /// Compose the existing member `labels` and opaque `context` fields into
    /// the shared surface metadata contract without changing the JSON shape.
    #[must_use]
    pub fn surface_metadata(&self) -> SurfaceMetadata {
        SurfaceMetadata::from_optional_parts(self.labels.clone(), self.context.clone())
    }

    /// Validate caller-supplied metadata for public member create surfaces.
    pub fn validate_public_surface_metadata(&self) -> Result<(), SurfaceMetadataError> {
        self.surface_metadata().validate_public()
    }
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

/// Closed wire stage for a per-identity `mob/reconcile` failure.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMobReconcileStage {
    Spawn,
    Retire,
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
    pub stage: WireMobReconcileStage,
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

/// Response payload for `mob/lifecycle`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobLifecycleResult {
    pub mob_id: String,
    pub action: WireMobLifecycleAction,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy_report: Option<Value>,
}

/// Request payload for `mob/append_system_context`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobAppendSystemContextParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Response payload for `mob/append_system_context`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobAppendSystemContextResult {
    pub mob_id: String,
    pub agent_identity: String,
    pub status: String,
}

/// Response payload for `mob/flows`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobFlowsResult {
    pub mob_id: String,
    pub flows: Vec<String>,
}

/// Request payload for `mob/flow_run`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFlowRunParams {
    pub mob_id: String,
    pub flow_id: String,
    #[serde(default)]
    pub params: Value,
}

/// Response payload for `mob/flow_run`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobFlowRunResult {
    pub run_id: String,
}

/// Request payload for `mob/flow_status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFlowStatusParams {
    pub mob_id: String,
    pub run_id: String,
}

/// Response payload for `mob/flow_status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobFlowStatusResult {
    pub run: Value,
}

/// Request payload for `mob/flow_cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobFlowCancelParams {
    pub mob_id: String,
    pub run_id: String,
}

/// Response payload for `mob/flow_cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobFlowCancelResult {
    pub canceled: bool,
}

/// Request payload for `mob/spawn_helper`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobSpawnHelperParams {
    pub mob_id: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
}

/// Request payload for `mob/fork_helper`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobForkHelperParams {
    pub mob_id: String,
    pub source_member_id: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_context: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
}

/// Response payload for `mob/spawn_helper` and `mob/fork_helper`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobHelperResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub tokens_used: u64,
    pub agent_identity: String,
    pub member_ref: WireMemberRef,
}

/// Response payload for `mob/force_cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobForceCancelResult {
    pub cancelled: bool,
}

/// Request payload for `mob/turn_start`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobTurnStartParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub prompt: WireContentInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
    #[serde(default)]
    pub clear_provider_params: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<WireAuthBindingRef>,
    #[serde(default)]
    pub clear_auth_binding: bool,
}

/// Response payload for `mob/member_status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobMemberStatusResult {
    pub status: WireMobMemberStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub tokens_used: u64,
    pub is_final: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_connectivity: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_member: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_capabilities: Option<crate::wire::WireResolvedModelCapabilities>,
}

/// Response payload for `mob/snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobSnapshotResult {
    pub mob_id: String,
    pub status: String,
    pub members: Vec<Value>,
}

#[cfg(test)]
mod member_status_capability_tests {
    use super::*;

    #[test]
    fn member_status_result_round_trips_resolved_capabilities() -> Result<(), serde_json::Error> {
        let capabilities = crate::wire::WireResolvedModelCapabilities {
            vision: true,
            image_input: true,
            image_tool_results: false,
            inline_video: false,
            realtime: true,
            web_search: true,
            image_generation: true,
        };
        let result = MobMemberStatusResult {
            status: WireMobMemberStatus::Active,
            output_preview: None,
            error: None,
            tokens_used: 0,
            is_final: false,
            current_session_id: Some("session-1".to_string()),
            peer_connectivity: None,
            kickoff: None,
            external_member: None,
            resolved_capabilities: Some(capabilities.clone()),
        };

        let json = serde_json::to_string(&result)?;
        assert!(json.contains("\"resolved_capabilities\""));
        let parsed: MobMemberStatusResult = serde_json::from_str(&json)?;
        assert_eq!(parsed.resolved_capabilities, Some(capabilities));
        Ok(())
    }
}

/// Response payload for `mob/destroy`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobDestroyResult {
    pub mob_id: String,
    pub ok: bool,
    pub destroy_report: Value,
}

/// Response payload for `mob/rotate_supervisor`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobRotateSupervisorResult {
    pub mob_id: String,
    pub ok: bool,
    pub report: SupervisorRotationReportWire,
}

/// Confirmed supervisor rotation report returned by `mob/rotate_supervisor`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SupervisorRotationReportWire {
    pub previous_epoch: u64,
    pub current_epoch: u64,
    pub public_peer_id: String,
}

/// Shared request payload for mob readiness waits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobWaitParams {
    pub mob_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Response payload for `mob/wait_kickoff` and `mob/wait_ready`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobWaitMembersResult {
    pub members: Vec<Value>,
}

/// Response payload for `mob/cancel_work`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobCancelWorkResult {
    pub mob_id: String,
    pub ok: bool,
}

/// Response payload for `mob/cancel_all_work`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobCancelAllWorkResult {
    pub mob_id: String,
    pub ok: bool,
}

/// Request payload for `mob/profile/create`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobProfileCreateParams {
    pub name: String,
    pub profile: MobProfileInput,
}

/// Request payload for `mob/profile/get`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobProfileNameParams {
    pub name: String,
}

/// Request payload for `mob/profile/update`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobProfileUpdateParams {
    pub name: String,
    pub profile: MobProfileInput,
    pub expected_revision: u64,
}

/// Request payload for `mob/profile/delete`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobProfileDeleteParams {
    pub name: String,
    pub expected_revision: u64,
}

/// Stored realm profile projection returned by `mob/profile/*`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobProfileLookupResult {
    #[serde(default)]
    pub not_found: bool,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Response payload for `mob/profile/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobProfileListResult {
    pub profiles: Vec<MobProfileLookupResult>,
}

/// Response payload for `mob/profile/delete`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobProfileDeleteResult {
    pub name: String,
    pub deleted_revision: u64,
}

/// Request payload for `mob/stream_open`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobStreamOpenParams {
    pub mob_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
}

/// Response payload for `mob/stream_open`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobStreamOpenResult {
    pub stream_id: String,
    pub opened: bool,
}

/// Request payload for `mob/stream_close`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobStreamCloseParams {
    pub stream_id: String,
}

/// Response payload for `mob/stream_close`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobStreamCloseResult {
    pub stream_id: String,
    pub closed: bool,
    pub already_closed: bool,
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
#[allow(clippy::expect_used, clippy::panic)]
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
    fn mob_member_spec_exposes_shared_surface_metadata() {
        let spec = MobMemberSpecWire {
            profile: "worker".into(),
            agent_identity: "w1".into(),
            initial_message: None,
            runtime_mode: None,
            backend: None,
            binding: None,
            context: Some(serde_json::json!({"client_ref": "member-card"})),
            labels: Some(BTreeMap::from([("client.member_id".into(), "w1".into())])),
            additional_instructions: None,
            auto_wire_parent: None,
        };

        let metadata = spec.surface_metadata();
        assert_eq!(
            metadata.labels.get("client.member_id").map(String::as_str),
            Some("w1")
        );
        assert_eq!(
            metadata.app_context,
            Some(serde_json::json!({"client_ref": "member-card"}))
        );
    }

    #[test]
    fn mob_member_spec_surface_metadata_rejects_reserved_keys() {
        let spec = MobMemberSpecWire {
            profile: "worker".into(),
            agent_identity: "w1".into(),
            initial_message: None,
            runtime_mode: None,
            backend: None,
            binding: None,
            context: None,
            labels: Some(BTreeMap::from([("mob_id".into(), "spoof".into())])),
            additional_instructions: None,
            auto_wire_parent: None,
        };

        assert!(spec.validate_public_surface_metadata().is_err());
    }

    #[test]
    fn mob_reconcile_failure_stage_is_typed_wire_enum() {
        let failure = MobReconcileFailureWire {
            agent_identity: "worker-1".into(),
            stage: WireMobReconcileStage::Spawn,
            error: "spawn failed".into(),
        };

        let json = serde_json::to_value(&failure).expect("serialize failure");
        assert_eq!(json["stage"], "spawn");

        let round_trip: MobReconcileFailureWire =
            serde_json::from_value(json).expect("deserialize failure");
        assert_eq!(round_trip.stage, WireMobReconcileStage::Spawn);

        let err = serde_json::from_value::<MobReconcileFailureWire>(serde_json::json!({
            "agent_identity": "worker-1",
            "stage": "restart",
            "error": "bad stage"
        }))
        .expect_err("unknown reconcile stage must be rejected");
        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn mob_lifecycle_params_reject_unknown_action_string() {
        let err = serde_json::from_value::<MobLifecycleParams>(serde_json::json!({
            "mob_id": "mob-1",
            "action": "explode"
        }))
        .expect_err("unknown lifecycle actions must fail at the typed wire boundary");

        assert!(
            err.to_string().contains("unknown variant"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_lifecycle_result_round_trips_typed_action() {
        let result = MobLifecycleResult {
            mob_id: "mob-1".into(),
            action: WireMobLifecycleAction::Complete,
            ok: true,
            destroy_report: None,
        };

        let json = serde_json::to_value(&result).expect("serialize lifecycle result");
        assert_eq!(json["action"], "complete");

        let round_trip: MobLifecycleResult =
            serde_json::from_value(json).expect("deserialize lifecycle result");
        assert_eq!(round_trip.action, WireMobLifecycleAction::Complete);
    }

    #[test]
    fn mob_wire_members_batch_contract_is_local_edge_native() {
        let params: MobWireMembersBatchParams = serde_json::from_value(serde_json::json!({
            "mob_id": "mob-1",
            "edges": [
                { "a": "lead", "b": "worker-b" },
                { "a": "worker-a", "b": "lead" }
            ]
        }))
        .expect("batch wire params deserialize");

        assert_eq!(params.mob_id, "mob-1");
        assert_eq!(params.edges.len(), 2);
        assert_eq!(params.edges[0].a, "lead");
        assert_eq!(params.edges[0].b, "worker-b");

        let result = MobWireMembersBatchResult {
            requested: 2,
            wired: vec![MobWireMembersBatchEdge {
                a: "lead".into(),
                b: "worker-a".into(),
            }],
            already_wired: vec![MobWireMembersBatchEdge {
                a: "lead".into(),
                b: "worker-b".into(),
            }],
        };
        let json = serde_json::to_value(&result).expect("serialize batch wire result");
        assert_eq!(json["requested"], 2);
        assert_eq!(json["wired"][0]["a"], "lead");
        assert_eq!(json["already_wired"][0]["b"], "worker-b");

        let err = serde_json::from_value::<MobWireMembersBatchParams>(serde_json::json!({
            "mob_id": "mob-1",
            "edges": [{ "member": "lead", "peer": "worker-a" }]
        }))
        .expect_err("mixed local/external mob/wire shape must not deserialize");
        let message = err.to_string();
        assert!(
            message.contains("unknown field `member`") || message.contains("missing field `a`"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn mob_spawn_many_result_entry_uses_typed_status_result_envelope() {
        let member_ref = WireMemberRef::encode("mob-1", "worker-1");
        let entry = MobSpawnManyResultEntry::spawned("worker-1", member_ref.clone());

        let json = serde_json::to_value(&entry).expect("serialize typed spawn_many row");
        assert_eq!(json["status"], "spawned");
        assert_eq!(json["result"]["agent_identity"], "worker-1");
        assert_eq!(json["result"]["member_ref"], member_ref.as_str());
        assert!(json.get("ok").is_none());
        assert!(json.get("error").is_none());

        let round_trip: MobSpawnManyResultEntry =
            serde_json::from_value(json).expect("deserialize typed spawn_many row");
        assert_eq!(round_trip, entry);

        let failed = MobSpawnManyResultEntry::failed(
            MobSpawnManyFailureCause::ProfileNotFound,
            "profile missing",
        );
        let json = serde_json::to_value(&failed).expect("serialize typed failed spawn_many row");
        assert_eq!(json["status"], "failed");
        assert_eq!(json["result"]["cause"], "profile_not_found");
        assert_eq!(json["result"]["message"], "profile missing");
        assert!(json.get("ok").is_none());
        assert!(json.get("error").is_none());

        let round_trip: MobSpawnManyResultEntry =
            serde_json::from_value(json).expect("deserialize typed failed spawn_many row");
        assert_eq!(round_trip, failed);
    }

    #[test]
    fn mob_spawn_many_result_entry_rejects_legacy_or_malformed_envelopes() {
        let legacy = serde_json::json!({
            "ok": true,
            "agent_identity": "worker-1",
            "member_ref": WireMemberRef::encode("mob-1", "worker-1"),
        });
        let err = serde_json::from_value::<MobSpawnManyResultEntry>(legacy)
            .expect_err("legacy ok carrier must not deserialize");
        assert!(
            err.to_string().contains("missing field `status`")
                || err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );

        let missing_result = serde_json::json!({
            "status": "spawned"
        });
        let err = serde_json::from_value::<MobSpawnManyResultEntry>(missing_result)
            .expect_err("missing typed result must fail closed");
        assert!(
            err.to_string().contains("missing field `result`"),
            "unexpected error: {err}"
        );

        let unknown_status = serde_json::json!({
            "status": "ok",
            "result": {
                "agent_identity": "worker-1",
                "member_ref": WireMemberRef::encode("mob-1", "worker-1"),
            }
        });
        let err = serde_json::from_value::<MobSpawnManyResultEntry>(unknown_status)
            .expect_err("unknown typed status must fail closed");
        assert!(
            err.to_string().contains("unknown variant"),
            "unexpected error: {err}"
        );

        let mismatched = serde_json::json!({
            "status": "spawned",
            "result": {
                "cause": "profile_not_found",
                "message": "profile missing"
            }
        });
        let err = serde_json::from_value::<MobSpawnManyResultEntry>(mismatched)
            .expect_err("status/result mismatch must fail closed");
        assert!(
            err.to_string()
                .contains("status spawned requires spawned result"),
            "unexpected error: {err}"
        );

        let message_only_failure = serde_json::json!({
            "status": "failed",
            "result": {
                "message": "profile missing"
            }
        });
        let err = serde_json::from_value::<MobSpawnManyResultEntry>(message_only_failure)
            .expect_err("string-only failure result must fail closed");
        assert!(
            err.to_string().contains("data did not match any variant")
                || err.to_string().contains("missing field `cause`"),
            "unexpected error: {err}"
        );

        let unknown_failure_cause = serde_json::json!({
            "status": "failed",
            "result": {
                "cause": "future_failure",
                "message": "future failure"
            }
        });
        let err = serde_json::from_value::<MobSpawnManyResultEntry>(unknown_failure_cause)
            .expect_err("unknown failure cause must fail closed");
        assert!(
            err.to_string().contains("data did not match any variant")
                || err.to_string().contains("unknown variant"),
            "unexpected error: {err}"
        );
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
    fn mob_wire_params_accept_canonical_external_peer_identity() {
        let params = serde_json::from_value::<MobWireParams>(serde_json::json!({
            "mob_id": "mob-1",
            "member": "member-a",
            "peer": {
                "external": {
                    "name": "external-worker",
                    "address": "inproc://external-worker",
                    "identity": {
                        "kind": "ed25519_public_key",
                        "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
                    }
                }
            }
        }))
        .expect("canonical external peer identity should deserialize");

        let MobPeerTarget::External(spec) = params.peer else {
            panic!("expected external peer target");
        };
        assert_eq!(spec.name, "external-worker");
    }

    #[test]
    fn mob_wire_params_reject_raw_external_peer_id_shape() {
        let err = serde_json::from_value::<MobWireParams>(serde_json::json!({
            "mob_id": "mob-1",
            "member": "member-a",
            "peer": {
                "external": {
                    "name": "external-worker",
                    "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                    "address": "inproc://external-worker",
                    "pubkey": vec![7u8; 32]
                }
            }
        }))
        .expect_err("raw peer_id/pubkey external peer shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("peer_id") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn mob_wire_params_reject_missing_external_peer_pubkey_material() {
        let err = serde_json::from_value::<MobWireParams>(serde_json::json!({
            "mob_id": "mob-1",
            "member": "member-a",
            "peer": {
                "external": {
                    "name": "external-worker",
                    "address": "inproc://external-worker",
                    "identity": {
                        "kind": "ed25519_public_key"
                    }
                }
            }
        }))
        .expect_err("missing external peer pubkey material must fail closed");

        let msg = err.to_string();
        assert!(
            msg.contains("public_key") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn runtime_binding_accepts_canonical_external_peer_identity() {
        let binding = serde_json::from_value::<WireRuntimeBinding>(serde_json::json!({
            "kind": "external",
            "address": "inproc://external-worker",
            "identity": {
                "kind": "ed25519_public_key",
                "public_key": "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
            }
        }))
        .expect("canonical external runtime binding identity should deserialize");

        let WireRuntimeBinding::External {
            identity, address, ..
        } = binding
        else {
            panic!("expected external runtime binding");
        };
        assert_eq!(address, "inproc://external-worker");
        assert_eq!(
            identity.resolve().expect("identity resolves").pubkey,
            [7u8; 32]
        );
    }

    #[test]
    fn runtime_binding_rejects_raw_external_peer_id_shape() {
        let err = serde_json::from_value::<WireRuntimeBinding>(serde_json::json!({
            "kind": "external",
            "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
            "address": "inproc://external-worker",
            "pubkey": vec![7u8; 32]
        }))
        .expect_err("raw peer_id/pubkey external runtime binding shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("peer_id") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn runtime_binding_rejects_missing_external_peer_pubkey_material() {
        let err = serde_json::from_value::<WireRuntimeBinding>(serde_json::json!({
            "kind": "external",
            "address": "inproc://external-worker",
            "identity": {
                "kind": "ed25519_public_key"
            }
        }))
        .expect_err("missing external runtime binding pubkey material must fail closed");

        let msg = err.to_string();
        assert!(
            msg.contains("public_key") || msg.contains("identity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn mob_turn_start_params_capture_turn_override_fields() {
        let params = serde_json::from_value::<MobTurnStartParams>(serde_json::json!({
            "mob_id": "mob-1",
            "agent_identity": "worker",
            "prompt": "continue",
            "output_schema": { "type": "object" },
            "structured_output_retries": 2
        }))
        .expect("turn_start should accept explicit turn override fields");

        assert_eq!(params.mob_id, "mob-1");
        assert_eq!(params.agent_identity, "worker");
        assert_eq!(params.prompt, WireContentInput::Text("continue".into()));
        assert_eq!(
            params.output_schema,
            Some(serde_json::json!({ "type": "object" }))
        );
        assert_eq!(params.structured_output_retries, Some(2));

        let err = serde_json::from_value::<MobTurnStartParams>(serde_json::json!({
            "mob_id": "mob-1",
            "agent_identity": "worker",
            "prompt": "continue",
            "unknown_override": true
        }))
        .expect_err("turn_start must reject unknown override fields");
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
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
