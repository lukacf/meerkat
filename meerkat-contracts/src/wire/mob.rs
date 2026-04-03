//! Mob RPC wire contracts.

use super::session::WireContentInput;
use meerkat_core::{
    HandlingMode, SessionId,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
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
/// bookkeeping fields such as `owner_session_id`, `session_cleanup_policy`,
/// `is_implicit`, and internal-only profile tool bundles are intentionally not
/// part of this schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobDefinitionInput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<MobOrchestratorInput>,
    pub profiles: BTreeMap<String, MobProfileInput>,
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTrustedPeerSpec {
    pub name: String,
    pub peer_id: String,
    pub address: String,
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
    pub meerkat_id: String,
    pub content: WireContentInput,
    #[serde(default)]
    pub handling_mode: WireHandlingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<WireRenderMetadata>,
}

/// Response payload for host-side mob member delivery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobMemberSendResult {
    pub mob_id: String,
    pub member_id: String,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

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
                "owner_session_id": "session-123",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        }))
        .expect_err("reserved runtime lifecycle fields must be rejected");

        assert!(
            err.to_string().contains("unknown field `owner_session_id`"),
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

        assert!(
            err.to_string().contains("unknown field `rust_bundles`"),
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
