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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Expose the realm-scoped WorkGraph commitment tools to this member.
    #[serde(default)]
    pub workgraph: bool,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Boxed: `Profile` is large; keeps the untagged wire shape unchanged.
    Inline(Box<Profile>),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileSource {
    /// Reference a realm-scoped reusable profile by name.
    RealmProfile {
        /// Name of the realm profile.
        name: String,
    },
    /// Inline profile definition.
    /// Boxed: `Profile` is large; keeps the tagged wire shape unchanged.
    Inline(Box<Profile>),
}

/// Profile template for spawning mob members.
///
/// Each profile defines the model, skills, tool configuration, and
/// communication properties for a class of mob members.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Profile {
    /// LLM model name (e.g. "claude-opus-4-8").
    pub model: String,
    /// Explicit typed provider for this profile's model.
    ///
    /// Parsed fail-closed at profile ingress into the closed
    /// [`meerkat_core::Provider`] vocabulary (unknown names reject the
    /// profile). Required for uncatalogued model ids that no registry entry
    /// owns; for catalogued ids the registry rejects a conflicting owner at
    /// build time.
    #[serde(default)]
    pub provider: Option<meerkat_core::Provider>,
    /// Durable self-hosted server binding for configured self-hosted aliases.
    ///
    /// Only meaningful together with `provider = "self_hosted"` and a
    /// `[self_hosted.models]` entry in the host config.
    #[serde(default)]
    pub self_hosted_server_id: Option<String>,
    /// Configured default provider for `Auto` image-generation targets.
    ///
    /// Overrides the mob-level default. When neither is set, `Auto` resolves
    /// via the session's effective text provider.
    #[serde(default)]
    pub image_generation_provider: Option<meerkat_core::Provider>,
    /// Per-profile auto-compaction threshold override (tokens, non-zero).
    ///
    /// `NonZeroU64` fails closed at ingress: a zero threshold rejects the
    /// profile instead of silently disabling compaction. When set, this wins
    /// over the global config knob and model-aware context-window scaling.
    #[serde(default)]
    pub auto_compact_threshold: Option<std::num::NonZeroU64>,
    /// Profile fields that win over durable session metadata on resume.
    ///
    /// Surfaces the typed [`meerkat_core::service::ResumeOverrideMask`]: a
    /// listed field is re-applied from the (possibly updated) profile when a
    /// durable member session resumes, instead of being restored from
    /// persisted metadata. Unlisted fields keep durable truth.
    #[serde(default)]
    pub resume_overrides: Vec<ResumeOverrideField>,
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
    /// When set, the agent session is configured with an `OutputSchema` that
    /// forces the LLM to respond with validated JSON conforming to this schema.
    ///
    /// Typed owner: a validated, normalized [`meerkat_core::MeerkatSchema`].
    /// The schema is validated ONCE at profile ingress — deserialization fails
    /// closed on an invalid schema (non-object root) — so a profile holding an
    /// invalid schema can no longer be persisted or transmitted and rejected
    /// only later at spawn time.
    #[serde(default, deserialize_with = "deserialize_output_schema")]
    pub output_schema: Option<meerkat_core::MeerkatSchema>,
    /// Optional provider-specific parameters passed to the LLM adapter.
    ///
    /// This maps directly to `AgentBuildConfig.provider_params` and is useful
    /// for model/provider knobs such as Gemini `thinking_budget` or OpenAI
    /// `reasoning_effort`.
    #[serde(default)]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
}

/// Profile fields that may override durable session metadata on resume.
///
/// Typed, closed vocabulary parsed fail-closed at profile ingress; maps onto
/// the corresponding bits of [`meerkat_core::service::ResumeOverrideMask`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResumeOverrideField {
    /// Re-apply the profile `model` on resume.
    Model,
    /// Re-apply the profile `provider` (and self-hosted binding) on resume.
    Provider,
    /// Re-apply the profile `provider_params` on resume.
    ProviderParams,
}

impl Profile {
    /// Project the declared `resume_overrides` into the typed core mask.
    pub fn resume_override_mask(&self) -> meerkat_core::service::ResumeOverrideMask {
        let mut mask = meerkat_core::service::ResumeOverrideMask::default();
        for field in &self.resume_overrides {
            match field {
                ResumeOverrideField::Model => mask.model = true,
                ResumeOverrideField::Provider => mask.provider = true,
                ResumeOverrideField::ProviderParams => mask.provider_params = true,
            }
        }
        mask
    }
}

/// Validate-at-ingress deserializer for [`Profile::output_schema`]: the raw
/// JSON is parsed into a [`meerkat_core::MeerkatSchema`] exactly once, failing
/// closed on an invalid schema instead of ferrying an unvalidated `Value`
/// through persistence and wire until spawn time.
fn deserialize_output_schema<'de, D>(
    deserializer: D,
) -> Result<Option<meerkat_core::MeerkatSchema>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<serde_json::Value>::deserialize(deserializer)?
        .map(|value| meerkat_core::MeerkatSchema::new(value).map_err(serde::de::Error::custom))
        .transpose()
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
            workgraph: true,
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
            workgraph: false,
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
            model: "claude-opus-4-8".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec!["orchestrator-skill".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                workgraph: true,
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
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec!["worker-skill".to_string()],
            tools: ToolConfig {
                builtins: false,
                shell: true,
                comms: true,
                memory: false,
                workgraph: false,
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
        assert!(!config.workgraph);
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
    fn test_profile_output_schema_validates_at_ingress() {
        // A well-formed object schema parses into the typed owner, normalized
        // once at deserialization.
        let profile: Profile = serde_json::from_str(
            r#"{"model":"claude-sonnet-4-5","output_schema":{"type":"object"}}"#,
        )
        .unwrap();
        assert!(profile.output_schema.is_some());

        // Regression: an invalid schema (non-object root) fails CLOSED at
        // profile ingress — it can no longer be persisted/transmitted and
        // rejected only later at spawn time.
        assert!(
            serde_json::from_str::<Profile>(
                r#"{"model":"claude-sonnet-4-5","output_schema":"not an object"}"#,
            )
            .is_err(),
            "an invalid output_schema must be rejected at profile deserialization"
        );
    }

    #[test]
    fn test_profile_toml_parses_typed_provider_and_self_hosted_binding() {
        let toml_str = r#"
model = "claude-internal-preview"
provider = "anthropic"
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.provider, Some(meerkat_core::Provider::Anthropic));
        assert_eq!(profile.self_hosted_server_id, None);

        let toml_str = r#"
model = "gemma-4-31b"
provider = "self_hosted"
self_hosted_server_id = "local"
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.provider, Some(meerkat_core::Provider::SelfHosted));
        assert_eq!(profile.self_hosted_server_id.as_deref(), Some("local"));
    }

    #[test]
    fn test_profile_provider_parse_is_fail_closed() {
        let toml_str = r#"
model = "claude-internal-preview"
provider = "antropic"
"#;
        assert!(
            toml::from_str::<Profile>(toml_str).is_err(),
            "unknown provider names must reject the profile at ingress"
        );
    }

    #[test]
    fn test_profile_toml_parses_image_generation_provider() {
        let toml_str = r#"
model = "claude-opus-4-8"
image_generation_provider = "gemini"
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            profile.image_generation_provider,
            Some(meerkat_core::Provider::Gemini)
        );
    }

    #[test]
    fn test_profile_auto_compact_threshold_rejects_zero() {
        let toml_str = r#"
model = "claude-opus-4-8"
auto_compact_threshold = 0
"#;
        assert!(
            toml::from_str::<Profile>(toml_str).is_err(),
            "zero compaction threshold must fail closed at profile ingress"
        );

        let profile: Profile = toml::from_str(
            r#"
model = "claude-opus-4-8"
auto_compact_threshold = 60000
"#,
        )
        .unwrap();
        assert_eq!(
            profile.auto_compact_threshold,
            std::num::NonZeroU64::new(60_000)
        );
    }

    #[test]
    fn test_profile_resume_overrides_parse_and_project_to_mask() {
        let toml_str = r#"
model = "claude-opus-4-8"
resume_overrides = ["model", "provider"]
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            profile.resume_overrides,
            vec![ResumeOverrideField::Model, ResumeOverrideField::Provider]
        );
        let mask = profile.resume_override_mask();
        assert!(mask.model);
        assert!(mask.provider);
        assert!(!mask.provider_params);
        assert!(!mask.max_tokens);
    }

    #[test]
    fn test_profile_resume_overrides_reject_unknown_fields() {
        let toml_str = r#"
model = "claude-opus-4-8"
resume_overrides = ["model", "everything"]
"#;
        assert!(
            toml::from_str::<Profile>(toml_str).is_err(),
            "resume_overrides vocabulary is closed; unknown entries must fail"
        );
    }

    #[test]
    fn test_profile_toml_parses_provider_params() {
        // K2: profile provider_params parse fail-closed into the typed
        // `ProviderParamsOverride` carrier at profile ingress.
        let toml_str = r#"
model = "gemini-3-pro-preview"
provider_params = { provider_tag = { provider = "gemini", thinking_budget = 8192, top_k = 20 } }
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            profile.provider_params,
            Some(
                meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                    provider_tag: Some(
                        meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(
                            meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
                                thinking_budget: Some(8192),
                                top_k: Some(20),
                                ..Default::default()
                            },
                        )
                    ),
                    ..Default::default()
                }
            )
        );
    }

    #[test]
    fn test_profile_toml_rejects_legacy_flat_provider_params() {
        // K2 fail-closed: the retired flat JSON-bag form is rejected at
        // profile parse, not at the first LLM call.
        let toml_str = r#"
model = "gemini-3-pro-preview"
provider_params = { thinking_budget = 8192, top_k = 20 }
"#;
        toml::from_str::<Profile>(toml_str)
            .expect_err("legacy flat provider_params must fail profile parse");
    }

    // -----------------------------------------------------------------------
    // ProfileBinding
    // -----------------------------------------------------------------------

    #[test]
    fn profile_binding_inline_roundtrip() {
        let profile = Profile {
            model: "claude-opus-4-8".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            ..Profile {
                model: String::new(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
        let binding = ProfileBinding::Inline(Box::new(profile.clone()));
        let json = serde_json::to_string(&binding).unwrap();
        let parsed: ProfileBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_inline().unwrap().model, "claude-opus-4-8");
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
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
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
            source: Box::new(ProfileSource::Inline(Box::new(profile))),
            allow_overlay: None,
            deny_overlay: None,
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: SpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }
}
