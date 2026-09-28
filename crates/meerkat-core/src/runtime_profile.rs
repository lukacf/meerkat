//! Shared vocabulary for runtime composition capability refusals.
//! Concrete profile policy is owned by meerkat-capabilities.

use serde::{Deserialize, Serialize};

/// Capabilities whose availability differs between runtime compositions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileCapability {
    InMemoryPersistence,
    DurablePersistence,
    ForegroundExecution,
    BackgroundExecution,
    KeepAlive,
    Comms,
    TransientTurnContext,
    Shell,
    ProcessSpawn,
    McpStdio,
    RemoteMemberPlacement,
    Hooks,
    RuntimeSkills,
    FileSchemaResolution,
    EmbeddedSkills,
    Schedule,
    WorkGraph,
    SemanticMemory,
    ImageGeneration,
    FallbackWebSearch,
    McpClient,
    TcpComms,
    UdsComms,
}

/// Canonical identity of a runtime capability profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileId {
    Browser,
}

/// Caller action that clears a profile refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfileClearingAction {
    UsePersistentRuntime,
    UseBackgroundRuntime,
    UseHostProcessRuntime,
    UseLocalMemberPlacement,
    UseHookRuntime,
    UseSkillRuntime,
    UseInlineSchema,
    UseScheduleRuntime,
    UseWorkGraphRuntime,
    UseSemanticMemoryRuntime,
    UseImageGenerationRuntime,
    UseWebSearchRuntime,
    UseMcpRuntime,
    UseInProcessComms,
}

/// Stable failure classification for an excluded runtime capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeProfileRefusalCode {
    CapabilityUnavailable,
}

/// Remediation facts attached to a runtime profile refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeProfileRefusalData {
    pub profile: RuntimeProfileId,
    pub capability: RuntimeProfileCapability,
    pub clearing_action: RuntimeProfileClearingAction,
}

/// Profile-owned refusal for mechanical transport serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeProfileRefusal {
    pub code: RuntimeProfileRefusalCode,
    pub message: String,
    pub data: RuntimeProfileRefusalData,
}

impl std::fmt::Display for RuntimeProfileRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for RuntimeProfileRefusal {}
