//! Profile and tool configuration for mob members.
//!
//! A `Profile` defines the template for spawning a member: which model to use,
//! which skills to load, tool configuration, and communication settings.

use crate::backend::MobBackendKind;
use crate::runtime_mode::MobRuntimeMode;
use serde::{Deserialize, Serialize};

/// Tool configuration for a mob member profile.
///
/// Controls which tool categories are enabled for members spawned
/// from this profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Enable built-in tools (file read, etc.).
    #[serde(default)]
    pub builtins: bool,
    /// Enable shell execution tool.
    #[serde(default)]
    pub shell: bool,
    /// Enable comms tools (peer messaging).
    #[serde(default)]
    pub comms: bool,
    /// Enable memory/semantic search tools.
    #[serde(default)]
    pub memory: bool,
    /// Enable mob management tools (spawn, retire, wire, unwire, list).
    #[serde(default)]
    pub mob: bool,
    /// Enable schedule tools (create, list, update, pause, resume, delete).
    #[serde(default)]
    pub schedule: bool,
    /// Enable assistant image generation tools.
    #[serde(default)]
    pub image_generation: bool,
    /// MCP server names this profile connects to.
    #[serde(default)]
    pub mcp: Vec<String>,
    /// Named Rust tool bundles wired by the mob runtime.
    ///
    /// String names referencing `Arc<dyn AgentToolDispatcher>` instances
    /// registered at mob construction time. Not serializable — must be
    /// re-registered on resume.
    #[serde(default)]
    pub rust_bundles: Vec<String>,
}

/// Binding for a profile in a mob definition.
///
/// Profiles can be defined inline (the existing behavior) or reference
/// a reusable realm-scoped profile by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProfileBinding {
    /// Reference to a realm-scoped profile by name.
    /// Must be listed before `Inline` for correct untagged deserialization
    /// (a `{"realm_profile":"x"}` object must not be consumed as an `Inline` variant).
    RealmRef {
        /// Name of the realm profile to reference.
        realm_profile: String,
    },
    /// Inline profile definition (original behavior).
    Inline(Profile),
}

impl ProfileBinding {
    /// Returns the inline profile if this is an `Inline` binding.
    pub fn as_inline(&self) -> Option<&Profile> {
        match self {
            Self::Inline(p) => Some(p),
            Self::RealmRef { .. } => None,
        }
    }

    /// Returns a mutable reference to the inline profile.
    pub fn as_inline_mut(&mut self) -> Option<&mut Profile> {
        match self {
            Self::Inline(p) => Some(p),
            Self::RealmRef { .. } => None,
        }
    }

    /// Returns the realm profile name if this is a `RealmRef` binding.
    pub fn realm_ref_name(&self) -> Option<&str> {
        match self {
            Self::RealmRef { realm_profile } => Some(realm_profile),
            Self::Inline(_) => None,
        }
    }
}

/// Agent-owned spawn tooling mode for child members.
///
/// Controls how the child's tool surface is determined at spawn time.
/// External/public spawn remains role-based; this enum is for agent-owned spawns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SpawnTooling {
    /// Inherit the parent's currently visible tools (ToolScope snapshot).
    InheritParent {
        /// Optional allow-list overlay: narrows the inherited set to only these tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_overlay: Option<Vec<String>>,
        /// Optional deny-list overlay: removes these tools from the inherited set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deny_overlay: Option<Vec<String>>,
    },
    /// Minimal: only comms tools (send_message, send_request, send_response, peers).
    Minimal,
    /// Use a specific profile for model/tool resolution.
    Profile {
        /// Source of the profile.
        source: Box<ProfileSource>,
        /// Optional allow-list overlay.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_overlay: Option<Vec<String>>,
        /// Optional deny-list overlay.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deny_overlay: Option<Vec<String>>,
    },
}

/// Source of a profile for spawn tooling resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileSource {
    /// Reference a realm-scoped reusable profile by name.
    RealmProfile {
        /// Name of the realm profile.
        name: String,
    },
    /// Inline profile definition.
    Inline(Profile),
}

/// Profile template for spawning mob members.
///
/// Each profile defines the model, skills, tool configuration, and
/// communication properties for a class of mob members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// LLM model name (e.g. "claude-opus-4-6").
    pub model: String,
    /// Skill references to load for this profile.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Tool configuration.
    #[serde(default)]
    pub tools: ToolConfig,
    /// Human-readable description of this member's role, visible to peers.
    #[serde(default)]
    pub peer_description: String,
    /// Whether this member can receive turns from external callers.
    #[serde(default)]
    pub external_addressable: bool,
    /// Optional backend override for this profile.
    ///
    /// If unset, runtime uses `definition.backend.default`.
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
    /// Runtime mode for members spawned from this profile.
    ///
    /// Defaults to autonomous keep-alive behavior when omitted.
    #[serde(default)]
    pub runtime_mode: MobRuntimeMode,
    /// Maximum peer-count threshold for inline peer lifecycle context injection.
    ///
    /// - `None`: use runtime default
    /// - `0`: never inline peer lifecycle notifications
    /// - `-1`: always inline peer lifecycle notifications
    /// - `>0`: inline only when post-drain peer count is <= threshold
    /// - `<-1`: invalid
    #[serde(default)]
    pub max_inline_peer_notifications: Option<i32>,
    /// Optional JSON Schema for structured output extraction.
    ///
    /// When set, the agent session is configured with an [`OutputSchema`] that
    /// forces the LLM to respond with validated JSON conforming to this schema.
    /// The value should be a valid JSON Schema object (root must be an object).
    ///
    /// **Note:** Validation is deferred to spawn time (`build_session_config`)
    /// where `MeerkatSchema::new()` rejects invalid schemas. This is intentional:
    /// `Profile` is a serializable template that may be persisted or transmitted
    /// before any agent is spawned, and `MeerkatSchema` does not currently
    /// implement `Eq` or validate on deserialization.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Optional provider-specific parameters passed to the LLM adapter.
    ///
    /// This maps directly to `AgentBuildConfig.provider_params` and is useful
    /// for model/provider knobs such as Gemini `thinking_budget` or OpenAI
    /// `reasoning_effort`.
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_config_serde_roundtrip() {
        let config = ToolConfig {
            builtins: true,
            shell: false,
            comms: true,
            memory: false,
            mob: true,
            schedule: true,
            image_generation: true,
            mcp: vec!["server-a".to_string(), "server-b".to_string()],
            rust_bundles: vec!["custom-tools".to_string()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ToolConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn test_tool_config_toml_roundtrip() {
        let config = ToolConfig {
            builtins: true,
            shell: true,
            comms: false,
            memory: false,
            mob: false,
            schedule: false,
            image_generation: false,
            mcp: vec!["mcp-server".to_string()],
            rust_bundles: Vec::new(),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ToolConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn test_profile_serde_roundtrip() {
        let profile = Profile {
            model: "claude-opus-4-6".to_string(),
            skills: vec!["orchestrator-skill".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                mob: true,
                schedule: false,
                image_generation: false,
                mcp: vec![],
                rust_bundles: vec![],
            },
            peer_description: "Orchestrates worker agents".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        };
        let json = serde_json::to_string(&profile).unwrap();
        let parsed: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, profile);
    }

    #[test]
    fn test_profile_toml_roundtrip() {
        let profile = Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["worker-skill".to_string()],
            tools: ToolConfig {
                builtins: false,
                shell: true,
                comms: true,
                memory: false,
                mob: false,
                schedule: false,
                image_generation: false,
                mcp: vec!["code-server".to_string()],
                rust_bundles: vec!["custom".to_string()],
            },
            peer_description: "Writes code".to_string(),
            external_addressable: false,
            backend: Some(MobBackendKind::External),
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: Some(20),
            output_schema: None,
            provider_params: None,
        };
        let toml_str = toml::to_string(&profile).unwrap();
        let parsed: Profile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, profile);
    }

    #[test]
    fn test_tool_config_defaults() {
        let config = ToolConfig::default();
        assert!(!config.builtins);
        assert!(!config.shell);
        assert!(!config.comms);
        assert!(!config.memory);
        assert!(!config.mob);
        assert!(!config.schedule);
        assert!(config.mcp.is_empty());
        assert!(config.rust_bundles.is_empty());
    }

    #[test]
    fn test_profile_default_fields_from_toml() {
        let toml_str = r#"
model = "claude-sonnet-4-5"
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.model, "claude-sonnet-4-5");
        assert!(profile.skills.is_empty());
        assert_eq!(profile.tools, ToolConfig::default());
        assert_eq!(profile.peer_description, "");
        assert!(!profile.external_addressable);
        assert_eq!(profile.backend, None);
        assert_eq!(profile.runtime_mode, MobRuntimeMode::AutonomousHost);
        assert_eq!(profile.max_inline_peer_notifications, None);
        assert_eq!(profile.provider_params, None);
    }

    #[test]
    fn test_profile_toml_parses_zero_inline_threshold() {
        let toml_str = r#"
model = "claude-sonnet-4-5"
max_inline_peer_notifications = 0
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.max_inline_peer_notifications, Some(0));
    }

    #[test]
    fn test_profile_toml_parses_always_inline_threshold() {
        let toml_str = r#"
model = "claude-sonnet-4-5"
max_inline_peer_notifications = -1
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.max_inline_peer_notifications, Some(-1));
    }

    #[test]
    fn test_profile_toml_parses_provider_params() {
        let toml_str = r#"
model = "gemini-3-pro-preview"
provider_params = { thinking_budget = 8192, top_k = 20 }
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            profile.provider_params,
            Some(serde_json::json!({"thinking_budget": 8192, "top_k": 20}))
        );
    }

    // -----------------------------------------------------------------------
    // ProfileBinding
    // -----------------------------------------------------------------------

    #[test]
    fn profile_binding_inline_roundtrip() {
        let profile = Profile {
            model: "claude-opus-4-6".to_string(),
            ..Profile {
                model: String::new(),
                skills: vec![],
                tools: ToolConfig::default(),
                peer_description: String::new(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }
        };
        let binding = ProfileBinding::Inline(profile.clone());
        let json = serde_json::to_string(&binding).unwrap();
        let parsed: ProfileBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_inline().unwrap().model, "claude-opus-4-6");
    }

    #[test]
    fn profile_binding_realm_ref_roundtrip() {
        let binding = ProfileBinding::RealmRef {
            realm_profile: "worker-v2".to_string(),
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(json.contains("realm_profile"));
        let parsed: ProfileBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.realm_ref_name(), Some("worker-v2"));
        assert!(parsed.as_inline().is_none());
    }

    #[test]
    fn profile_binding_backward_compat_raw_profile_deserializes_as_inline() {
        // A raw Profile JSON (no realm_profile key) should deserialize as Inline
        let profile_json = r#"{"model":"claude-sonnet-4-5"}"#;
        let binding: ProfileBinding = serde_json::from_str(profile_json).unwrap();
        assert!(binding.as_inline().is_some());
        assert_eq!(binding.as_inline().unwrap().model, "claude-sonnet-4-5");
    }

    #[test]
    fn profile_binding_realm_ref_not_confused_with_inline() {
        // A realm_profile-only object should NOT be consumed as Inline
        let ref_json = r#"{"realm_profile":"my-profile"}"#;
        let binding: ProfileBinding = serde_json::from_str(ref_json).unwrap();
        assert!(binding.realm_ref_name().is_some());
        assert!(binding.as_inline().is_none());
    }

    // -----------------------------------------------------------------------
    // SpawnTooling
    // -----------------------------------------------------------------------

    #[test]
    fn spawn_tooling_inherit_parent_roundtrip() {
        let tooling = SpawnTooling::InheritParent {
            allow_overlay: Some(vec!["shell".into()]),
            deny_overlay: Some(vec!["memory_search".into()]),
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: SpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn spawn_tooling_minimal_roundtrip() {
        let tooling = SpawnTooling::Minimal;
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: SpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn spawn_tooling_profile_realm_roundtrip() {
        let tooling = SpawnTooling::Profile {
            source: Box::new(ProfileSource::RealmProfile {
                name: "worker-v2".into(),
            }),
            allow_overlay: None,
            deny_overlay: Some(vec!["dangerous_tool".into()]),
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: SpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn spawn_tooling_profile_inline_roundtrip() {
        let profile = Profile {
            model: "claude-sonnet-4-5".into(),
            skills: vec![],
            tools: ToolConfig::default(),
            peer_description: String::new(),
            external_addressable: false,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        };
        let tooling = SpawnTooling::Profile {
            source: Box::new(ProfileSource::Inline(profile)),
            allow_overlay: None,
            deny_overlay: None,
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: SpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }
}
