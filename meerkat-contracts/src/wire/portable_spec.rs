//! Portable member specification — the digest-covered payload of
//! `MaterializeMember` (multi-host mobs plan §14.2/§15.3).
//!
//! Fail-closed by construction: secret-bearing VALUES (shell env, MCP stdio
//! env, MCP HTTP header values), non-portable carriers (rust bundles,
//! per-spawn external tools, host-surface MCP allowlists), launch modes, and
//! generation/fence/digest bookkeeping are structurally unrepresentable here.
//! Launch mode and the idempotency tuple ride the `MaterializeMember` command
//! as siblings of the spec (A6/A12) — the spec has no second carrier for any
//! of them. Every struct is `deny_unknown_fields` so a payload that smuggles
//! one of the excluded facts fails at the serde boundary instead of being
//! silently ignored.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::connection::WireAuthBindingRef;
use super::mob::{WireMobResumeOverrideField, WireMobRuntimeMode};
use super::runtime::WireProviderParamsOverride;
use super::supervisor_bridge::WireOpaqueJson;

/// Fully resolved, host-independent member build specification.
///
/// `comms_name` and realm identity are deliberately absent: they are
/// re-derived member-side through their single owners (`MemberCommsName`,
/// `meerkat_core::mob_realm_id`). The kickoff/initial message is absent:
/// it is delivered on the first real turn via `DeliverMemberInput`.
///
/// `Eq` is load-bearing: the spec rides `MaterializeMember` inside the
/// bridge command `Eq` chain (see [`PortableProfile`]'s manual `Eq` note).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableMemberSpec {
    pub mob_id: String,
    pub profile_name: String,
    pub agent_identity: String,
    /// Fully resolved profile: realm-ref/override-profile/inherited-filter
    /// category-opening already applied controlling-host-side.
    pub profile: PortableProfile,
    pub definition_extract: PortableDefinitionExtract,
    pub overlay: PortableSpawnOverlay,
    /// Env key NAMES the member host must be able to satisfy locally.
    /// Values never travel (R5); a missing key is a typed
    /// `EnvKeyMissing` materialize rejection, never a silent default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_env_keys: Vec<String>,
}

/// Portable projection of `meerkat_mob::Profile`.
///
/// `provider` is REQUIRED (R4): the member host never re-infers a provider
/// for the model id. There is no `backend` field — placement is not profile
/// vocabulary; `RuntimeBinding` is machine-owned.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableProfile {
    pub model: String,
    pub provider: meerkat_core::Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_hosted_server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_generation_provider: Option<meerkat_core::Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_compact_threshold: Option<std::num::NonZeroU64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resume_overrides: Vec<WireMobResumeOverrideField>,
    /// Skill NAMES; the content travels in
    /// [`PortableDefinitionExtract::skills`] as inline sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: PortableToolConfig,
    #[serde(default)]
    pub peer_description: String,
    #[serde(default)]
    pub external_addressable: bool,
    #[serde(default)]
    pub runtime_mode: WireMobRuntimeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_peer_notifications: Option<i32>,
    /// Validated `MeerkatSchema`, string-enveloped (unlike
    /// `WireMobProfile`'s bare-`Value` carrier) so the spec keeps the
    /// bridge `Eq` chain; the member host parses + re-validates at build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<WireOpaqueJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<WireProviderParamsOverride>,
}

/// `Eq` is required by the `MaterializeMember` command chain, and
/// `provider_params` holds `f32` knobs, so `Eq` cannot be derived. The
/// manual claim is sound on this wire: JSON has no NaN literal, and
/// `serde_json` refuses to serialize non-finite floats — so a value whose
/// derived `PartialEq` could violate reflexivity can neither be decoded
/// from nor encoded to the wire this type exists for.
impl Eq for PortableProfile {}

/// Portable projection of `meerkat_mob::ToolConfig` category toggles.
///
/// Structural exclusions vs the domain type: no `rust_bundles` carrier
/// (in-process `Arc` dispatchers cannot travel) and no `mcp` name allowlist
/// (it gates the HOST-surface external-tools provider, which is
/// non-portable). `non_portable_disabled` is a reserved typed projection;
/// v1 rejects every non-portable category instead of silently disabling it,
/// so production v1 specs carry an empty list.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableToolConfig {
    #[serde(default)]
    pub builtins: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub comms: bool,
    #[serde(default)]
    pub memory: bool,
    #[serde(default)]
    pub workgraph: bool,
    #[serde(default)]
    pub mob: bool,
    #[serde(default)]
    pub schedule: bool,
    #[serde(default)]
    pub image_generation: bool,
    /// Declarative MCP servers keyed by server name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_servers: BTreeMap<String, PortableMcpDecl>,
    /// Reserved typed record; production v1 always emits an empty list (A4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_portable_disabled: Vec<WireNonPortableResourceKind>,
}

/// Portable MCP server declaration (A10).
///
/// Mirrors `meerkat_core::mcp_config` transports MINUS the secret-bearing
/// value maps: stdio `env` and HTTP `headers` are structurally absent; only
/// the required key/header NAMES travel, satisfied member-host-side.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableMcpDecl {
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required_env_keys: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_timeout_secs: Option<u64>,
    },
    Http {
        url: String,
        /// URL transport subtype. An absent value preserves the historical
        /// default of streamable HTTP; explicit SSE must survive placement.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_transport: Option<meerkat_core::mcp_config::McpHttpTransport>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required_header_names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_timeout_secs: Option<u64>,
    },
}

/// Definition-level facts the member build needs beyond the profile.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableDefinitionExtract {
    /// Mob-scoped custom model registry entries. Reuses the typed config
    /// owner — the same carrier `MobDefinitionInput.models` already puts on
    /// the wire.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, meerkat_core::config::CustomModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_generation_provider: Option<meerkat_core::Provider>,
    /// Referenced skills only; `Path` sources are resolved to `Inline`
    /// controlling-host-side and a read failure fails the spawn closed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<String, PortableSkillSource>,
    /// Profile-name roster for default spawn-profile grant parity checks
    /// only; spawn authority itself ships minted in the overlay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_names: Vec<String>,
}

/// Skill source that can travel: inline content only. A filesystem `Path`
/// variant is structurally unrepresentable — paths are meaningless on the
/// member host.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableSkillSource {
    Inline { content: String },
}

/// Per-spawn overlay resolved controlling-host-side.
///
/// No launch-mode field: `MaterializeMember.launch` is the only launch
/// carrier (A6/§19.L1) and the domain `SpawnMemberSpec.launch_mode` never
/// serializes into this spec.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableSpawnOverlay {
    /// Opaque application context pass-through (same carrier class as
    /// `MobSpawnParams.context`; not a semantic fact), string-enveloped
    /// for the bridge `Eq` chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<WireOpaqueJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    pub system_prompt: PortableSystemPrompt,
    /// Resolved policy only — `Inherit` is unrepresentable (O3); `None`
    /// means no per-member policy, not "inherit something ambient".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<WireResolvedToolAccessPolicy>,
    /// Minted by the controlling host; shipped for member-side visibility
    /// composition only. Dispatch authority is re-resolved
    /// controlling-host-side (R6) — this projection is never a second
    /// authority source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_tool_authority_context: Option<WireMobToolAuthorityContext>,
    /// Name-ref only; zero credential material travels (D3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<WireAuthBindingRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<meerkat_core::BudgetLimits>,
    pub runtime_mode: WireMobRuntimeMode,
    #[serde(default)]
    pub continuity_intent: WireSpawnContinuityIntent,
}

/// Resolved system prompt for the member build. `Inherit` is
/// unrepresentable (R3): the controlling host resolves inheritance before
/// the spec is minted, so the member host never guesses.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "prompt", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableSystemPrompt {
    Set { text: String },
    Disable,
}

/// Resolved tool access policy — mirrors `WireToolAccessPolicy`'s
/// allow/deny shape exactly, with `Inherit` structurally absent (O3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum WireResolvedToolAccessPolicy {
    AllowList(Vec<String>),
    DenyList(Vec<String>),
}

/// Wire mirror of `meerkat_mob::SpawnContinuityIntent` (same serde shape).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireSpawnContinuityIntent {
    #[default]
    Ephemeral,
    DurableIdentity {
        continuity_key: String,
    },
}

/// Wire PROJECTION of the sealed `meerkat_core::service::MobToolAuthorityContext`.
///
/// Carries exactly the serde-visible facts the sealed domain type exposes
/// (its private generation seal never serializes). Receivers rehydrate
/// through the documented lane; this projection cannot mint authority.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMobToolAuthorityContext {
    /// Opaque principal token — compared as a blob, never decoded.
    pub principal_token: String,
    pub can_create_mobs: bool,
    #[serde(default)]
    pub can_mutate_profiles: bool,
    #[serde(default)]
    pub can_run_adaptive_packs: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub managed_mob_scope: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spawn_profile_scope: BTreeMap<String, BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_provenance: Option<WireMobToolCallerProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_invocation_id: Option<String>,
}

/// Wire mirror of `meerkat_core::service::MobToolCallerProvenance`
/// (informational projection only — never a policy input).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMobToolCallerProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_mob_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_member_id: Option<String>,
}

/// Merged vocabulary of resources that cannot cross hosts (A4).
///
/// No `ScheduleTools` kind (DEC-6): A3-final permits schedule tools on
/// remote members, so no producer for that kind can exist — the wire and
/// machine vocabularies agree on these six.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireNonPortableResourceKind {
    RustBundles,
    PerSpawnExternalTools,
    MobDefaultExternalTools,
    DefaultLlmClientOverride,
    HostSurfaceMcpAllowlist,
    WorkgraphTools,
}

/// Vocabulary of secret-bearing fields the portability admission rejects
/// (A2/R5) — named typed so the rejection cause can say which one.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSecretBearingFieldKind {
    ShellEnv,
    McpStdioEnv,
    McpHttpHeaders,
}

#[cfg(test)]
pub(crate) fn sample_portable_member_spec() -> PortableMemberSpec {
    PortableMemberSpec {
        mob_id: "mob-1".to_string(),
        profile_name: "worker".to_string(),
        agent_identity: "worker-1".to_string(),
        profile: PortableProfile {
            model: "claude-opus-4-8".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            image_generation_provider: Some(meerkat_core::Provider::Gemini),
            auto_compact_threshold: std::num::NonZeroU64::new(60_000),
            resume_overrides: vec![WireMobResumeOverrideField::Model],
            skills: vec!["review".to_string()],
            tools: PortableToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                workgraph: false,
                mob: true,
                schedule: false,
                image_generation: false,
                mcp_servers: BTreeMap::from([(
                    "docs".to_string(),
                    PortableMcpDecl::Stdio {
                        command: "docs-server".to_string(),
                        args: vec!["--stdio".to_string()],
                        required_env_keys: vec!["DOCS_TOKEN".to_string()],
                        connect_timeout_secs: Some(10),
                    },
                )]),
                non_portable_disabled: vec![WireNonPortableResourceKind::RustBundles],
            },
            peer_description: "reviews things".to_string(),
            external_addressable: false,
            runtime_mode: WireMobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: Some(3),
            output_schema: None,
            provider_params: None,
        },
        definition_extract: PortableDefinitionExtract {
            models: BTreeMap::new(),
            image_generation_provider: None,
            skills: BTreeMap::from([(
                "review".to_string(),
                PortableSkillSource::Inline {
                    content: "# Review skill".to_string(),
                },
            )]),
            profile_names: vec!["worker".to_string()],
        },
        overlay: PortableSpawnOverlay {
            context: Some(WireOpaqueJson::from_value(
                &serde_json::json!({"client_ref": "card-7"}),
            )),
            labels: Some(BTreeMap::from([("team".to_string(), "alpha".to_string())])),
            additional_instructions: Some(vec!["Be terse.".to_string()]),
            system_prompt: PortableSystemPrompt::Set {
                text: "You are worker-1.".to_string(),
            },
            tool_access_policy: Some(WireResolvedToolAccessPolicy::AllowList(vec![
                "member_status".to_string(),
            ])),
            mob_tool_authority_context: Some(WireMobToolAuthorityContext {
                principal_token: "principal-token-1".to_string(),
                can_create_mobs: false,
                can_mutate_profiles: false,
                can_run_adaptive_packs: false,
                managed_mob_scope: BTreeSet::from(["mob-1".to_string()]),
                spawn_profile_scope: BTreeMap::from([(
                    "mob-1".to_string(),
                    BTreeSet::from(["worker".to_string()]),
                )]),
                caller_provenance: Some(WireMobToolCallerProvenance {
                    caller_session_id: Some("sess-1".to_string()),
                    caller_mob_id: Some("mob-1".to_string()),
                    caller_member_id: Some("lead".to_string()),
                }),
                audit_invocation_id: Some("audit-1".to_string()),
            }),
            auth_binding: None,
            budget_limits: Some(meerkat_core::BudgetLimits {
                max_tokens: Some(100_000),
                max_duration: None,
                max_tool_calls: Some(50),
            }),
            runtime_mode: WireMobRuntimeMode::TurnDriven,
            continuity_intent: WireSpawnContinuityIntent::Ephemeral,
        },
        required_env_keys: vec!["DOCS_TOKEN".to_string()],
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn portable_member_spec_full_fixture_round_trips() {
        let spec = sample_portable_member_spec();
        let value = serde_json::to_value(&spec).expect("serialize spec");
        let decoded: PortableMemberSpec =
            serde_json::from_value(value.clone()).expect("decode spec");
        let reencoded = serde_json::to_value(&decoded).expect("reserialize spec");
        assert_eq!(
            value, reencoded,
            "PortableMemberSpec round-trip must preserve wire shape"
        );
    }

    #[test]
    fn portable_member_spec_rejects_unknown_top_level_fields() {
        let mut value =
            serde_json::to_value(sample_portable_member_spec()).expect("serialize spec");
        value["unexpected"] = json!(true);
        assert!(
            serde_json::from_value::<PortableMemberSpec>(value).is_err(),
            "unknown top-level fields must fail closed"
        );
    }

    /// Structural pins (A6/A12/R5): launch mode, the idempotency tuple, the
    /// digest, and shell env have no carrier inside the spec — a payload
    /// smuggling any of them is a decode error, not an ignored key.
    #[test]
    fn portable_member_spec_has_no_launch_generation_fence_digest_or_shell_env_carrier() {
        for (field, sample) in [
            ("launch_mode", json!({"mode": "fresh"})),
            ("launch", json!({"mode": "fresh"})),
            ("generation", json!(3)),
            ("fence_token", json!(9)),
            ("spec_digest", json!("abc")),
            ("shell_env", json!({"PATH": "/usr/bin"})),
        ] {
            let mut value =
                serde_json::to_value(sample_portable_member_spec()).expect("serialize spec");
            value[field] = sample;
            assert!(
                serde_json::from_value::<PortableMemberSpec>(value).is_err(),
                "field {field} must be structurally unrepresentable on PortableMemberSpec"
            );
        }
    }

    #[test]
    fn portable_profile_rejects_unknown_and_missing_provider() {
        let spec = sample_portable_member_spec();
        let mut value = serde_json::to_value(&spec.profile).expect("serialize profile");
        value["backend"] = json!("session");
        assert!(
            serde_json::from_value::<PortableProfile>(value).is_err(),
            "profile backend is not portable vocabulary and must fail closed"
        );

        let mut value = serde_json::to_value(&spec.profile).expect("serialize profile");
        value
            .as_object_mut()
            .expect("profile object")
            .remove("provider");
        assert!(
            serde_json::from_value::<PortableProfile>(value).is_err(),
            "provider is REQUIRED on the portable profile (R4)"
        );
    }

    #[test]
    fn portable_tool_config_rejects_rust_bundles_and_host_mcp_allowlist() {
        for (field, sample) in [
            ("rust_bundles", json!(["internal"])),
            ("mcp", json!(["host-server"])),
        ] {
            let mut value = json!({});
            value[field] = sample;
            assert!(
                serde_json::from_value::<PortableToolConfig>(value).is_err(),
                "non-portable carrier {field} must fail closed on PortableToolConfig"
            );
        }
    }

    #[test]
    fn portable_mcp_decl_rejects_secret_value_maps() {
        let stdio_with_env = json!({
            "transport": "stdio",
            "command": "docs-server",
            "env": { "DOCS_TOKEN": "secret" }
        });
        assert!(
            serde_json::from_value::<PortableMcpDecl>(stdio_with_env).is_err(),
            "stdio env VALUES are secret-bearing and structurally absent (A10)"
        );

        let http_with_headers = json!({
            "transport": "http",
            "url": "https://mcp.example",
            "headers": { "authorization": "Bearer x" }
        });
        assert!(
            serde_json::from_value::<PortableMcpDecl>(http_with_headers).is_err(),
            "http header VALUES are secret-bearing and structurally absent (A10)"
        );

        let decl: PortableMcpDecl = serde_json::from_value(json!({
            "transport": "http",
            "url": "https://mcp.example",
            "http_transport": "sse",
            "required_header_names": ["authorization"],
            "connect_timeout_secs": 23
        }))
        .expect("name-only http decl decodes");
        assert_eq!(
            decl,
            PortableMcpDecl::Http {
                url: "https://mcp.example".to_string(),
                http_transport: Some(meerkat_core::mcp_config::McpHttpTransport::Sse),
                required_header_names: vec!["authorization".to_string()],
                connect_timeout_secs: Some(23),
            }
        );

        let legacy: PortableMcpDecl = serde_json::from_value(json!({
            "transport": "http",
            "url": "https://mcp.example"
        }))
        .expect("legacy http declaration keeps the streamable-http default");
        assert_eq!(
            legacy,
            PortableMcpDecl::Http {
                url: "https://mcp.example".to_string(),
                http_transport: None,
                required_header_names: Vec::new(),
                connect_timeout_secs: None,
            }
        );
    }

    #[test]
    fn portable_system_prompt_inherit_is_unrepresentable() {
        assert!(
            serde_json::from_value::<PortableSystemPrompt>(json!({"prompt": "inherit"})).is_err(),
            "inherit must not decode as a portable system prompt (R3)"
        );
        let set: PortableSystemPrompt =
            serde_json::from_value(json!({"prompt": "set", "text": "hi"})).expect("set decodes");
        assert_eq!(
            set,
            PortableSystemPrompt::Set {
                text: "hi".to_string()
            }
        );
        let disable: PortableSystemPrompt =
            serde_json::from_value(json!({"prompt": "disable"})).expect("disable decodes");
        assert_eq!(disable, PortableSystemPrompt::Disable);
    }

    #[test]
    fn portable_skill_source_path_is_unrepresentable() {
        assert!(
            serde_json::from_value::<PortableSkillSource>(
                json!({"source": "path", "path": "/tmp/skill.md"})
            )
            .is_err(),
            "path skill sources must not decode (resolved to inline controlling-host-side)"
        );
        let inline: PortableSkillSource =
            serde_json::from_value(json!({"source": "inline", "content": "# s"}))
                .expect("inline decodes");
        assert_eq!(
            inline,
            PortableSkillSource::Inline {
                content: "# s".to_string()
            }
        );
    }

    #[test]
    fn wire_resolved_tool_access_policy_inherit_is_unrepresentable() {
        for inherit in [
            json!({"type": "inherit"}),
            json!({"type": "Inherit"}),
            json!("inherit"),
        ] {
            assert!(
                serde_json::from_value::<WireResolvedToolAccessPolicy>(inherit.clone()).is_err(),
                "inherit shape {inherit} must fail decode (O3)"
            );
        }
        for policy in [
            WireResolvedToolAccessPolicy::AllowList(vec!["a".to_string()]),
            WireResolvedToolAccessPolicy::DenyList(vec!["b".to_string()]),
        ] {
            let value = serde_json::to_value(&policy).expect("serialize policy");
            let decoded: WireResolvedToolAccessPolicy =
                serde_json::from_value(value).expect("decode policy");
            assert_eq!(decoded, policy);
        }
    }

    #[test]
    fn non_portable_and_secret_vocabularies_round_trip_snake_case() {
        let kinds: &[(WireNonPortableResourceKind, &str)] = &[
            (WireNonPortableResourceKind::RustBundles, "rust_bundles"),
            (
                WireNonPortableResourceKind::PerSpawnExternalTools,
                "per_spawn_external_tools",
            ),
            (
                WireNonPortableResourceKind::MobDefaultExternalTools,
                "mob_default_external_tools",
            ),
            (
                WireNonPortableResourceKind::DefaultLlmClientOverride,
                "default_llm_client_override",
            ),
            (
                WireNonPortableResourceKind::HostSurfaceMcpAllowlist,
                "host_surface_mcp_allowlist",
            ),
            (
                WireNonPortableResourceKind::WorkgraphTools,
                "workgraph_tools",
            ),
        ];
        assert_eq!(
            kinds.len(),
            6,
            "six non-portable kinds — schedule tools are portable (DEC-6)"
        );
        for (kind, expected) in kinds {
            let value = serde_json::to_value(kind).expect("serialize kind");
            assert_eq!(value, json!(expected));
            let decoded: WireNonPortableResourceKind =
                serde_json::from_value(value).expect("decode kind");
            assert_eq!(decoded, *kind);
        }
        assert!(
            serde_json::from_value::<WireNonPortableResourceKind>(json!("schedule_tools")).is_err(),
            "schedule_tools must not decode — schedule tools cross hosts (DEC-6)"
        );

        let fields: &[(WireSecretBearingFieldKind, &str)] = &[
            (WireSecretBearingFieldKind::ShellEnv, "shell_env"),
            (WireSecretBearingFieldKind::McpStdioEnv, "mcp_stdio_env"),
            (
                WireSecretBearingFieldKind::McpHttpHeaders,
                "mcp_http_headers",
            ),
        ];
        for (kind, expected) in fields {
            let value = serde_json::to_value(kind).expect("serialize field kind");
            assert_eq!(value, json!(expected));
            let decoded: WireSecretBearingFieldKind =
                serde_json::from_value(value).expect("decode field kind");
            assert_eq!(decoded, *kind);
        }
    }

    #[test]
    fn overlay_rejects_unknown_nested_fields() {
        let spec = sample_portable_member_spec();
        let mut value = serde_json::to_value(&spec.overlay).expect("serialize overlay");
        value["launch_mode"] = json!({"mode": "fresh"});
        assert!(
            serde_json::from_value::<PortableSpawnOverlay>(value).is_err(),
            "overlay must reject a smuggled launch-mode carrier (A6)"
        );
    }
}
