//! REST wire contracts.
//!
//! REST handlers consume these request bodies directly. OpenAPI emission
//! derives schemas from the same types so the published REST contract cannot
//! drift from the production deserializer.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use meerkat_core::{
    BudgetLimits, ContentInput, HookRunOverrides, OutputSchema, PeerMeta, Provider,
    service::TurnToolOverlay,
    skills::{SkillKey, SkillRef},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    PeerResponseTerminalStatusWire, WireContentBlock, WireForkContext, WireMobBackendKind,
    WireMobRuntimeMode,
};

/// Request body for `POST /sessions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestCreateSessionRequest {
    pub prompt: ContentInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Cow<'static, str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    /// Max retries for structured output validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Keep session alive after turn completes, listening for comms messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
    /// Optional run-scoped hook overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable built-in tools. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_builtins: Option<bool>,
    /// Enable shell tool. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_shell: Option<bool>,
    /// Enable semantic memory. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_memory: Option<bool>,
    /// Enable mob tools. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_mob: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_web_search: Option<bool>,
    /// Enable WorkGraph tools. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_workgraph: Option<bool>,
    /// Explicit budget limits for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<serde_json::Value>"))]
    pub budget_limits: Option<BudgetLimits>,
    /// Provider-specific parameters (for example reasoning config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
    /// Skills to preload into the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload_skills: Option<Vec<SkillKey>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_refs: Option<Vec<SkillRef>>,
    /// Optional key-value labels attached at session creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Additional instruction sections appended to the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Opaque application context passed through to custom builders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<HashMap<String, String>>,
}

/// Request body for `POST /sessions/{id}/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestContinueSessionRequest {
    pub session_id: String,
    pub prompt: ContentInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    /// Max retries for structured output validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
    /// Keep session alive after turn completes, listening for comms messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Cow<'static, str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Optional run-scoped hook overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable Meerkat-owned fallback web search. Omit to inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_web_search: Option<bool>,
    /// Structured refs for per-turn skill injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_refs: Option<Vec<SkillRef>>,
    /// Optional per-turn flow tool overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    /// Additional instruction sections prepended as system notices to the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
}

/// Request body for `POST /sessions/{id}/system_context`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestAppendSystemContextRequest {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Request body for `POST /sessions/{id}/external-events`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestSessionExternalEventEnvelope {
    GenericJson {
        event_type: String,
        payload: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<WireContentBlock>>,
    },
    PeerResponseTerminal {},
}

/// Request body for `POST /sessions/{id}/peer-response-terminal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RestPeerResponseTerminalRequest {
    pub peer_id: meerkat_core::comms::PeerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<meerkat_core::comms::PeerName>,
    pub request_id: meerkat_core::PeerCorrelationId,
    pub status: PeerResponseTerminalStatusWire,
    pub result: Value,
}

/// Request body for `POST /mob/{id}/spawn-helper`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestMobHelperRequest {
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

/// Request body for `POST /mob/{id}/fork-helper`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestMobForkHelperRequest {
    pub source_member_id: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_context: Option<WireForkContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
}

/// Optional request body for `POST /mob/{id}/wait-kickoff`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestMobWaitRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}
