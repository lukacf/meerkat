//! REST surface wire contracts.
//!
//! Canonical request/response bodies for the routes in
//! [`crate::rest_catalog`]. The emitted OpenAPI document derives component
//! schemas from these structs via `schema_for!`, so the advertised REST
//! contract is a projection of the same types the surface deserializes —
//! a field added here (or in a referenced core wire type) lands in the
//! schema automatically and hand-rolled drift is impossible.
//!
//! `meerkat-rest` deserializes these types at ingress; it owns no competing
//! body definitions.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use meerkat_core::skills::{SkillKey, SkillRef};
use meerkat_core::{
    BudgetLimits, Config, ContentInput, HookRunOverrides, OutputSchema, PeerCorrelationId,
    PeerMeta, Provider, PublicTurnToolOverlay,
    comms::{PeerId, PeerName},
    config::SystemPromptOverride,
    connection::{BindingId, ProfileId, RealmId},
    lifecycle::run_primitive::CoreRenderable,
};

use super::mob::{WireForkContext, WireMobBackendKind, WireMobRuntimeMode};
use super::runtime::PeerResponseTerminalStatusWire;

/// `POST /sessions` — create a session and run the first turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestCreateSessionRequest {
    pub prompt: ContentInput,
    /// Typed per-request system-prompt policy: omit/`null` to inherit, a
    /// string to set an explicit prompt, or `{"action": "disable"}` to
    /// suppress every prompt source.
    #[serde(default, skip_serializing_if = "SystemPromptOverride::is_inherit")]
    pub system_prompt: SystemPromptOverride,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    /// Realm-scoped provider credential binding. Same structural
    /// `WireAuthBindingRef` ({realm, binding, profile}); the server-owned
    /// `origin` provenance is reconstructed as Configured and never client input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<super::WireAuthBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    /// Max retries for structured output validation.
    /// Omit to use the product default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable,
    /// Some(false) = disable. Requires comms_name when enabled.
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
    /// Enable schedule tools. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_schedule: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_web_search: Option<bool>,
    /// Enable WorkGraph tools. Omit to use factory defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_workgraph: Option<bool>,
    /// Explicit budget limits for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<BudgetLimits>,
    /// Typed provider-specific parameter overrides (for example reasoning
    /// config). Parsed fail-closed at the wire boundary (K2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::wire::runtime::WireProviderParamsOverride>,
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

/// `POST /sessions/{id}/messages` — continue a session with a new message.
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
    /// Omit to inherit the current/persisted session value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output_retries: Option<u32>,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable,
    /// Some(false) = disable.
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
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    /// Realm-scoped provider credential binding override for this resumed
    /// turn. Omit to inherit the persisted session binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<super::WireAuthBindingRef>,
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
    /// Optional per-turn flow tool overlay (caller-safe public shape;
    /// runtime-owned dispatch metadata is not accepted at this boundary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tool_overlay: Option<PublicTurnToolOverlay>,
    /// Additional instruction sections prepended as system notices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
}

/// `POST /sessions/{id}/system_context` — append runtime system context.
///
/// The body is the typed [`CoreRenderable`] owner rather than a bare `text`
/// string; a plain-text client payload still deserializes via
/// `CoreRenderable`'s tagged `text` variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestAppendSystemContextRequest {
    pub content: CoreRenderable,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// `POST /sessions/{id}/peer-response-terminal` — admit a correlated
/// terminal peer response through the typed runtime ingress.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RestPeerResponseTerminalRequest {
    pub peer_id: PeerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<PeerName>,
    pub request_id: PeerCorrelationId,
    pub status: PeerResponseTerminalStatusWire,
    pub result: Value,
}

/// `PUT /config` — replace the config (bare config or wrapped with an
/// optimistic-concurrency generation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum RestSetConfigRequest {
    Wrapped {
        #[cfg_attr(
            feature = "schema",
            schemars(with = "crate::wire::config::ConfigContractSchema")
        )]
        config: Config,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_generation: Option<u64>,
    },
    Direct(
        #[cfg_attr(
            feature = "schema",
            schemars(with = "crate::wire::config::ConfigContractSchema")
        )]
        Config,
    ),
}

/// `PATCH /config` — RFC-7386 merge-patch (bare patch or wrapped with an
/// optimistic-concurrency generation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum RestPatchConfigRequest {
    Wrapped {
        patch: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_generation: Option<u64>,
    },
    Direct(Value),
}

/// `POST /auth/profiles` — store binding-scoped credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestAuthProfileCreateRequest {
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ProfileId>,
    pub provider: String,
    pub auth_method: String,
    pub secret: String,
}

/// `POST /auth/bindings/{binding_id}/test` — test a binding resolve path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestAuthBindingTestRequest {
    pub realm_id: RealmId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ProfileId>,
}

/// `POST /mob/{id}/spawn-helper` — spawn a short-lived helper member.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestMobHelperRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<super::WireAuthBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
}

/// `POST /mob/{id}/fork-helper` — fork a helper member from an existing one.
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
    pub auth_binding: Option<super::WireAuthBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_context: Option<WireForkContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<WireMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<WireMobBackendKind>,
}

/// `POST /mob/{id}/wait-kickoff` — wait for the kickoff completion barrier.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestMobWaitRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// `POST /mob/{id}/wire-members-batch` — wire multiple local member edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestMobWireMembersBatchRequest {
    pub edges: Vec<super::mob::MobWireMembersBatchEdge>,
}

/// `GET /sessions/{id}` — session details response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestSessionDetailsResponse {
    pub session_id: String,
    pub session_ref: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn create_session_request_carries_enable_web_search() {
        // Regression lock for the OpenAPI drift this module eliminates: the
        // REST create-session contract is this struct, and the schema is
        // derived from it, so `enable_web_search` (previously missing from
        // the hand-rolled component) is part of the advertised contract.
        let parsed: RestCreateSessionRequest = serde_json::from_value(serde_json::json!({
            "prompt": "hello",
            "enable_web_search": true,
        }))
        .expect("create-session body parses");
        assert_eq!(parsed.enable_web_search, Some(true));
    }

    #[test]
    fn create_session_request_carries_structural_auth_binding() {
        let parsed: RestCreateSessionRequest = serde_json::from_value(serde_json::json!({
            "prompt": "hello",
            "auth_binding": {
                "realm": "dev",
                "binding": "default_openai",
                "profile": "console"
            }
        }))
        .expect("create-session body parses");
        let auth_binding = parsed.auth_binding.expect("auth_binding should parse");
        assert_eq!(auth_binding.realm.as_str(), "dev");
        assert_eq!(auth_binding.binding.as_str(), "default_openai");
        assert_eq!(
            auth_binding
                .profile
                .as_ref()
                .map(|profile| profile.as_str()),
            Some("console")
        );
    }

    #[test]
    fn continue_session_request_carries_structural_auth_binding() {
        let parsed: RestContinueSessionRequest = serde_json::from_value(serde_json::json!({
            "session_id": "session-1",
            "prompt": "hello again",
            "auth_binding": {
                "realm": "dev",
                "binding": "default_openai"
            }
        }))
        .expect("continue-session body parses");
        let auth_binding = parsed.auth_binding.expect("auth_binding should parse");
        assert_eq!(auth_binding.realm.as_str(), "dev");
        assert_eq!(auth_binding.binding.as_str(), "default_openai");
        assert!(auth_binding.profile.is_none());
    }

    #[test]
    fn mob_helper_requests_carry_structural_auth_binding() {
        let parsed: RestMobHelperRequest = serde_json::from_value(serde_json::json!({
            "prompt": "help",
            "agent_identity": "helper",
            "auth_binding": {
                "realm": "dev",
                "binding": "default_anthropic",
                "profile": "console"
            }
        }))
        .expect("spawn helper body parses");
        let auth_binding = parsed.auth_binding.expect("auth_binding should parse");
        assert_eq!(auth_binding.realm.as_str(), "dev");
        assert_eq!(auth_binding.binding.as_str(), "default_anthropic");
        assert_eq!(
            auth_binding
                .profile
                .as_ref()
                .map(|profile| profile.as_str()),
            Some("console")
        );

        let parsed: RestMobForkHelperRequest = serde_json::from_value(serde_json::json!({
            "source_member_id": "source",
            "prompt": "help",
            "agent_identity": "helper",
            "auth_binding": {
                "realm": "dev",
                "binding": "default_anthropic"
            }
        }))
        .expect("fork helper body parses");
        let auth_binding = parsed.auth_binding.expect("auth_binding should parse");
        assert_eq!(auth_binding.realm.as_str(), "dev");
        assert_eq!(auth_binding.binding.as_str(), "default_anthropic");
        assert!(auth_binding.profile.is_none());
    }

    #[test]
    fn create_session_request_system_prompt_tri_state() {
        // The REST create contract carries the typed tri-state policy: string
        // sets, omission inherits, and the explicit disable object survives
        // parse (no lossy Disable→Inherit collapse at this boundary).
        let parsed: RestCreateSessionRequest = serde_json::from_value(serde_json::json!({
            "prompt": "hello",
            "system_prompt": "You are terse.",
        }))
        .expect("string system_prompt parses");
        assert_eq!(
            parsed.system_prompt,
            SystemPromptOverride::Set("You are terse.".to_string())
        );

        let parsed: RestCreateSessionRequest = serde_json::from_value(serde_json::json!({
            "prompt": "hello",
        }))
        .expect("omitted system_prompt parses");
        assert!(parsed.system_prompt.is_inherit());

        let parsed: RestCreateSessionRequest = serde_json::from_value(serde_json::json!({
            "prompt": "hello",
            "system_prompt": {"action": "disable"},
        }))
        .expect("disable system_prompt parses");
        assert_eq!(parsed.system_prompt, SystemPromptOverride::Disable);
    }

    #[test]
    fn peer_response_terminal_rejects_unknown_fields() {
        let err = serde_json::from_value::<RestPeerResponseTerminalRequest>(serde_json::json!({
            "peer_id": "00000000-0000-0000-0000-000000000000",
            "request_id": "00000000-0000-0000-0000-000000000000",
            "status": "completed",
            "result": {},
            "unexpected": true,
        }))
        .unwrap_err();
        assert!(err.to_string().contains("unexpected"));
    }
}
