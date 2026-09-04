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
    ///
    /// Must be `true` for every profile that spawns a member. A member's
    /// identity, roster entry, wiring, peer messaging, and supervisor bridge
    /// are keyed on its comms name, and the facade composes comms tools
    /// whenever that name is set, so `build_agent_config` rejects
    /// `comms = false` instead of silently ignoring it. The serde default is
    /// `false`, which means omitting the key is rejected too. A member that
    /// must not message peers keeps comms on and uses [`Self::read_only`] or a
    /// per-spawn `tool_access_policy` deny list.
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
    /// Declare this profile read-only: every tool call is admitted at the
    /// execution gate only when the dispatcher that owns the tool declares it
    /// read-only ([`meerkat_core::ToolMutationClass::ReadOnly`]).
    ///
    /// This is an enforcement declaration, not prompt guidance, and it wins
    /// over the per-spawn tool access policy (a spawn cannot widen a
    /// read-only profile). The category booleans above still decide what is
    /// *visible*; this decides what may *execute*. The guarantee covers only
    /// tools that traverse the dispatcher: see
    /// `meerkat_core::tool_execution_policy` for the exact boundary
    /// (provider-native tools, MCP tools, and `shell` are not read-only).
    #[serde(default)]
    pub read_only: bool,
    /// MCP server names this profile connects to.
    #[serde(default)]
    pub mcp: Vec<String>,
    /// Declarative MCP server configs for members built from this profile.
    ///
    /// Durable on the profile (unlike the in-process-only per-spawn
    /// `SpawnMemberSpec.external_tools` overlay), so revival recomposes the
    /// member's MCP tools from the same profile that built it. These are
    /// already profile-scoped; the `mcp` name allowlist above gates only the
    /// mob-wide default external-tools provider (where an empty list means
    /// the full host surface).
    #[serde(default)]
    pub mcp_servers: Vec<meerkat_core::mcp_config::McpServerConfig>,
    /// Named Rust tool bundles wired by the mob runtime.
    ///
    /// String names referencing `Arc<dyn AgentToolDispatcher>` instances
    /// registered at mob construction time. Not serializable — must be
    /// re-registered on resume.
    #[serde(default)]
    pub rust_bundles: Vec<String>,
}

impl ToolConfig {
    /// Every key a `[profiles.<name>.tools]` table may declare, in
    /// declaration order, exactly as the serde derive reads them.
    ///
    /// `ToolConfig` has no `deny_unknown_fields` for the same reason
    /// [`Profile`] has none (see [`Profile::FIELD_NAMES`]): it is the
    /// persisted profile shape. `MobDefinition::from_toml` compares the
    /// `tools` sub-table of each inline profile against this list so a typo
    /// such as `comm = true` is reported instead of silently leaving the
    /// category off. A serde-derived drift test keeps it equal to the
    /// derive's own field list.
    pub const FIELD_NAMES: &'static [&'static str] = &[
        "builtins",
        "shell",
        "comms",
        "memory",
        "workgraph",
        "mob",
        "schedule",
        "image_generation",
        "read_only",
        "mcp",
        "mcp_servers",
        "rust_bundles",
    ];
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

    /// The only key a realm-reference profile table declares.
    ///
    /// The untagged derive tries `RealmRef` first and, having no
    /// `deny_unknown_fields`, accepts a table that carries `realm_profile`
    /// plus anything else, dropping the rest. `MobDefinition::from_toml`
    /// therefore compares a realm-reference table against this list: a key
    /// from the closed [`UnsupportedProfileKey`] list refuses the parse
    /// exactly as for an inline profile, and any other extra key warns.
    pub const REALM_REF_FIELD_NAMES: &'static [&'static str] = &["realm_profile"];
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
    /// Minimal: only comms tools (send, send_message, reply_to_peer,
    /// send_request, send_response, peers).
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
    /// Human-readable description of this member's role, shown to peers in
    /// discovery: it becomes the member's `PeerMeta.description`, which other
    /// members read through the `peers` tool and the `mob.peer_added`
    /// lifecycle notice.
    ///
    /// NOT this member's system prompt. The member's own prompt is assembled
    /// from [`Self::skills`] resolved against the definition's `[skills.<id>]`
    /// tables (inline or path content); see the mobs guide, "Member system
    /// prompt". A profile has no prompt key, and `MobDefinition::from_toml`
    /// refuses one (see [`UnsupportedProfileKey`]).
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
    /// Every key a `[profiles.<name>]` table may declare, in declaration
    /// order, exactly as the serde derive reads them.
    ///
    /// `Profile` cannot carry `deny_unknown_fields`: it sits under the
    /// untagged [`ProfileBinding`], where a rejected key would surface as an
    /// opaque "did not match any variant" error, and it is also the persisted
    /// SQLite/JSON profile shape. `MobDefinition::from_toml` therefore
    /// compares each inline profile table's keys against this list after
    /// parsing. A serde-derived drift test keeps it equal to the derive's own
    /// field list.
    pub const FIELD_NAMES: &'static [&'static str] = &[
        "model",
        "provider",
        "self_hosted_server_id",
        "image_generation_provider",
        "auto_compact_threshold",
        "resume_overrides",
        "skills",
        "tools",
        "peer_description",
        "external_addressable",
        "backend",
        "runtime_mode",
        "max_inline_peer_notifications",
        "output_schema",
        "provider_params",
    ];

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

/// Profile keys that name a platform concept a mob profile cannot honour.
///
/// Each of these is an author's attempt to give the member its own prompt
/// text. A profile has no such field: the member prompt is `profile.skills`
/// resolved against `[skills.<id>]` tables, and identity-first hosts add
/// `DurableAgentSpec.additional_instructions` or a customizer's
/// `draft.system_prompt`. Because `Profile` cannot use `deny_unknown_fields`
/// (see [`Profile::FIELD_NAMES`]), such a key would otherwise be dropped
/// silently and the member would run on the default prompt.
/// `MobDefinition::from_toml` refuses these keys with
/// `MobError::UnsupportedProfileKey`; every other unknown key only warns, so
/// host-private keys keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedProfileKey {
    /// `system_prompt = "..."` under `[profiles.<name>]`.
    SystemPrompt,
    /// `prompt = "..."` under `[profiles.<name>]`.
    Prompt,
    /// `instructions = "..."` under `[profiles.<name>]`.
    Instructions,
}

impl UnsupportedProfileKey {
    /// The closed list of refused keys.
    pub const ALL: [Self; 3] = [Self::SystemPrompt, Self::Prompt, Self::Instructions];

    /// Where the concept every refused key reaches for actually lives.
    pub const HINT: &'static str = "a profile has no system_prompt; the member prompt is \
        `profile.skills` resolved against an inline or path `[skills.<id>]` table, and \
        identity-first hosts may set `DurableAgentSpec.additional_instructions` or a \
        customizer's `draft.system_prompt`";

    /// The refused key as it appears in TOML.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemPrompt => "system_prompt",
            Self::Prompt => "prompt",
            Self::Instructions => "instructions",
        }
    }

    /// Classify a raw profile key; `None` for every key that is not refused.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == key)
    }
}

impl std::fmt::Display for UnsupportedProfileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

    /// Capture the field list a struct's serde derive announces through
    /// `deserialize_struct`, without producing a value. `None` means the
    /// derive never entered `deserialize_struct`.
    fn derive_field_names<'de, T: serde::Deserialize<'de>>() -> Option<&'static [&'static str]> {
        use std::cell::Cell;

        /// A deserializer that never produces a value: it only records the
        /// field list the serde derive announces through `deserialize_struct`.
        struct FieldNameProbe<'a> {
            captured: &'a Cell<Option<&'static [&'static str]>>,
        }

        impl<'de> serde::Deserializer<'de> for FieldNameProbe<'_> {
            type Error = serde::de::value::Error;

            fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
            where
                V: serde::de::Visitor<'de>,
            {
                Err(serde::de::Error::custom(
                    "field-name probe: derive did not enter deserialize_struct",
                ))
            }

            fn deserialize_struct<V>(
                self,
                _name: &'static str,
                fields: &'static [&'static str],
                _visitor: V,
            ) -> Result<V::Value, Self::Error>
            where
                V: serde::de::Visitor<'de>,
            {
                self.captured.set(Some(fields));
                Err(serde::de::Error::custom(
                    "field-name probe: fields captured",
                ))
            }

            serde::forward_to_deserialize_any! {
                bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
                bytes byte_buf option unit unit_struct newtype_struct seq tuple
                tuple_struct map enum identifier ignored_any
            }
        }

        let captured = Cell::new(None);
        let probe = FieldNameProbe {
            captured: &captured,
        };
        assert!(
            T::deserialize(probe).is_err(),
            "the probe never yields a value"
        );
        captured.get()
    }

    #[test]
    fn profile_field_names_match_serde_derive() {
        assert_eq!(
            derive_field_names::<Profile>(),
            Some(Profile::FIELD_NAMES),
            "Profile::FIELD_NAMES drifted from the serde derive's field list \
             (None means the derive never entered deserialize_struct)"
        );
    }

    #[test]
    fn tool_config_field_names_match_serde_derive() {
        assert_eq!(
            derive_field_names::<ToolConfig>(),
            Some(ToolConfig::FIELD_NAMES),
            "ToolConfig::FIELD_NAMES drifted from the serde derive's field list \
             (None means the derive never entered deserialize_struct)"
        );
    }

    #[test]
    fn realm_ref_field_names_match_the_serialized_binding() {
        // The untagged enum never enters `deserialize_struct` for the variant,
        // so the drift check runs serializer-side: the keys a `RealmRef`
        // binding emits are exactly the keys a realm-reference table may
        // declare.
        let binding = ProfileBinding::RealmRef {
            realm_profile: "reviewer".to_string(),
        };
        let table = toml::Value::try_from(&binding).expect("realm ref serializes to TOML");
        let emitted: Vec<&str> = table
            .as_table()
            .expect("realm ref serializes as a table")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(emitted, ProfileBinding::REALM_REF_FIELD_NAMES);
        for key in UnsupportedProfileKey::ALL {
            assert!(
                !ProfileBinding::REALM_REF_FIELD_NAMES.contains(&key.as_str()),
                "{key} must not be a realm-reference key"
            );
        }
    }

    #[test]
    fn unsupported_profile_keys_are_a_closed_list_with_a_prompt_hint() {
        for key in UnsupportedProfileKey::ALL {
            assert_eq!(UnsupportedProfileKey::from_key(key.as_str()), Some(key));
            assert_eq!(key.to_string(), key.as_str());
            assert!(
                !Profile::FIELD_NAMES.contains(&key.as_str()),
                "{key} must not be a real profile field"
            );
        }
        assert_eq!(
            UnsupportedProfileKey::from_key("system_prompt"),
            Some(UnsupportedProfileKey::SystemPrompt)
        );
        assert_eq!(
            UnsupportedProfileKey::from_key("prompt"),
            Some(UnsupportedProfileKey::Prompt)
        );
        assert_eq!(
            UnsupportedProfileKey::from_key("instructions"),
            Some(UnsupportedProfileKey::Instructions)
        );
        // Host-private keys (HomeCore carries `role_summary` under every
        // profile table) and real fields are never refused.
        assert_eq!(UnsupportedProfileKey::from_key("role_summary"), None);
        assert_eq!(UnsupportedProfileKey::from_key("peer_description"), None);
        assert_eq!(
            UnsupportedProfileKey::from_key("additional_instructions"),
            None
        );
        for needle in [
            "a profile has no system_prompt",
            "`profile.skills`",
            "`[skills.<id>]`",
            "`DurableAgentSpec.additional_instructions`",
            "`draft.system_prompt`",
        ] {
            assert!(
                UnsupportedProfileKey::HINT.contains(needle),
                "hint must mention {needle}: {}",
                UnsupportedProfileKey::HINT
            );
        }
    }

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
            read_only: false,
            mcp: vec!["server-a".to_string(), "server-b".to_string()],
            mcp_servers: vec![],
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
            read_only: false,
            mcp: vec!["mcp-server".to_string()],
            mcp_servers: vec![],
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
                read_only: false,
                mcp: vec![],
                mcp_servers: vec![],
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
                read_only: false,
                mcp: vec!["code-server".to_string()],
                mcp_servers: vec![],
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
