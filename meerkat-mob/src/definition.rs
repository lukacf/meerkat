//! Mob definition types and TOML parsing.
//!
//! A `MobDefinition` describes the complete structure of a mob: profiles,
//! MCP servers, wiring rules, and skill sources. Definitions are serializable
//! so they can be stored in `MobCreated` events for resume recovery.

use crate::MobBackendKind;
use crate::ids::{BranchId, FlowId, FlowNodeId, LoopId, MobId, ProfileName, StepId};
use crate::profile::{Profile, ProfileBinding};
use indexmap::IndexMap;
use meerkat_core::types::ContentInput;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Orchestrator configuration within a mob definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Profile name of the orchestrator.
    pub profile: ProfileName,
}

/// MCP server configuration for a mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Stdio command to launch the server (mutually exclusive with `url`).
    #[serde(default)]
    pub command: Vec<String>,
    /// HTTP/SSE URL for the server (mutually exclusive with `command`).
    #[serde(default)]
    pub url: Option<String>,
    /// Environment variables passed to the server process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Source for a skill definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SkillSource {
    /// Inline skill content.
    Inline {
        /// Skill content text.
        content: String,
    },
    /// Skill loaded from a file path.
    Path {
        /// Path to the skill file.
        path: String,
    },
}

/// Wiring rule between two profile roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleWiringRule {
    /// First profile name.
    pub a: ProfileName,
    /// Second profile name.
    pub b: ProfileName,
}

/// Wiring rules controlling automatic peer connections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WiringRules {
    /// Automatically wire every spawned meerkat to the orchestrator.
    #[serde(default)]
    pub auto_wire_orchestrator: bool,
    /// Fan-out wiring rules between profile roles.
    #[serde(default)]
    pub role_wiring: Vec<RoleWiringRule>,
}

/// External backend configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalBackendConfig {
    /// Base address prefix used to publish external peer addresses.
    pub address_base: String,
}

/// Backend selection and backend-specific settings for the mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BackendConfig {
    /// Default backend used when a spawn call does not explicitly select one.
    #[serde(default)]
    pub default: MobBackendKind,
    /// External backend settings; required when external backend is selected.
    #[serde(default)]
    pub external: Option<ExternalBackendConfig>,
}

/// Runtime dispatch mode for a step.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
    #[default]
    FanOut,
    OneToOne,
    FanIn,
}

/// Aggregation policy for step outcomes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CollectionPolicy {
    #[default]
    All,
    Any,
    Quorum {
        n: u8,
    },
}

/// Dependency interpretation for a step.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyMode {
    #[default]
    All,
    Any,
}

/// How to parse a step target's terminal output.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutputFormat {
    /// Parse output as JSON.
    #[default]
    Json,
    /// Keep output as plain text (stored as a JSON string value).
    Text,
}

/// Predicate expression for a step guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ConditionExpr {
    Eq {
        path: String,
        value: serde_json::Value,
    },
    In {
        path: String,
        values: Vec<serde_json::Value>,
    },
    Gt {
        path: String,
        value: serde_json::Value,
    },
    Lt {
        path: String,
        value: serde_json::Value,
    },
    And {
        exprs: Vec<ConditionExpr>,
    },
    Or {
        exprs: Vec<ConditionExpr>,
    },
    Not {
        expr: Box<ConditionExpr>,
    },
}

/// A frame is a DAG of nodes that executes as a unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSpec {
    pub nodes: IndexMap<FlowNodeId, FlowNodeSpec>,
}

/// A node in a FrameSpec: either a step or a repeat_until loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowNodeSpec {
    Step(FrameStepSpec),
    RepeatUntil(RepeatUntilSpec),
}

/// A step node within a frame (like FlowStepSpec but scoped to a frame).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameStepSpec {
    pub step_id: StepId,
    pub depends_on: Vec<FlowNodeId>,
    pub depends_on_mode: DependencyMode,
    pub branch: Option<BranchId>,
}

/// A repeat_until loop node within a frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatUntilSpec {
    pub loop_id: LoopId,
    pub depends_on: Vec<FlowNodeId>,
    pub depends_on_mode: DependencyMode,
    pub body: FrameSpec,
    pub until: ConditionExpr,
    pub max_iterations: u32,
}

/// Per-step flow execution configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowStepSpec {
    pub role: ProfileName,
    pub message: ContentInput,
    #[serde(default)]
    pub depends_on: Vec<StepId>,
    #[serde(default)]
    pub dispatch_mode: DispatchMode,
    #[serde(default)]
    pub collection_policy: CollectionPolicy,
    #[serde(default)]
    pub condition: Option<ConditionExpr>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub expected_schema_ref: Option<String>,
    #[serde(default)]
    pub branch: Option<BranchId>,
    #[serde(default)]
    pub depends_on_mode: DependencyMode,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
    #[serde(default)]
    pub output_format: StepOutputFormat,
}

/// Flow definition for a named workflow.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FlowSpec {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub steps: IndexMap<StepId, FlowStepSpec>,
    /// v2 flows carry a FrameSpec as the execution root. v1 flows omit this field.
    #[serde(default)]
    pub root: Option<FrameSpec>,
}

/// Topology enforcement mode.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    #[default]
    Advisory,
    Strict,
}

/// Directed topology rule between roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyRule {
    pub from_role: ProfileName,
    pub to_role: ProfileName,
    pub allowed: bool,
}

/// Topology policy configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologySpec {
    pub mode: PolicyMode,
    pub rules: Vec<TopologyRule>,
}

/// Supervisor configuration for escalation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorSpec {
    pub role: ProfileName,
    pub escalation_threshold: u32,
}

/// Runtime guardrails for flow execution.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LimitsSpec {
    pub max_flow_duration_ms: Option<u64>,
    pub max_step_retries: Option<u32>,
    pub max_orphaned_turns: Option<u32>,
    #[serde(default)]
    pub cancel_grace_timeout_ms: Option<u64>,
    /// Maximum number of concurrently active nodes across all frames (0 = unlimited).
    #[serde(default)]
    pub max_active_nodes: Option<u64>,
    /// Maximum number of concurrently active body frames (0 = unlimited).
    #[serde(default)]
    pub max_active_frames: Option<u64>,
    /// Maximum nesting depth for body frames (0 = unlimited).
    #[serde(default)]
    pub max_frame_depth: Option<u64>,
}

/// Declarative spawn policy for automatic member provisioning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SpawnPolicyConfig {
    /// No automatic spawning.
    None,
    /// Automatically spawn members based on profile map.
    Auto {
        /// Maps target identifiers to profile names for auto-spawn resolution.
        profile_map: BTreeMap<String, ProfileName>,
    },
}

/// Declarative event router configuration for mob-wide event aggregation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRouterConfig {
    /// Channel buffer size for the event router. Defaults to 256.
    #[serde(default = "default_event_router_buffer_size")]
    pub buffer_size: usize,
    /// Event type patterns to include (if set, only matching events are routed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_patterns: Option<Vec<String>>,
    /// Event type patterns to exclude (applied after include filter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_patterns: Option<Vec<String>>,
}

fn default_event_router_buffer_size() -> usize {
    256
}

/// Canonical cleanup semantics for mobs indexed to an owning session.
///
/// `owner_session_id` remains lookup/indexing metadata only. Cleanup eligibility
/// is owned exclusively by this policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionCleanupPolicy {
    #[default]
    Manual,
    DestroyOnOwnerArchive,
}

impl SessionCleanupPolicy {
    pub fn is_manual(policy: &Self) -> bool {
        matches!(policy, Self::Manual)
    }
}

/// Complete mob definition.
///
/// Describes profiles, MCP servers, wiring rules, and skill sources.
/// Serializable for storage in `MobCreated` events. `rust_bundles` in
/// `ToolConfig` are stored as string names only; actual dispatchers
/// must be re-registered on resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobDefinition {
    /// Unique mob identifier.
    pub id: MobId,
    /// Optional orchestrator configuration.
    #[serde(default)]
    pub orchestrator: Option<OrchestratorConfig>,
    /// Named profiles for spawning meerkats.
    ///
    /// Each profile can be an inline definition or a reference to a
    /// realm-scoped reusable profile.
    #[serde(default)]
    pub profiles: BTreeMap<ProfileName, ProfileBinding>,
    /// Named MCP server configurations.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// Wiring rules for automatic peer connections.
    #[serde(default)]
    pub wiring: WiringRules,
    /// Named skill sources.
    #[serde(default)]
    pub skills: BTreeMap<String, SkillSource>,
    /// Backend selection defaults and backend-specific config.
    #[serde(default)]
    pub backend: BackendConfig,
    /// Named flow definitions.
    #[serde(default)]
    pub flows: BTreeMap<FlowId, FlowSpec>,
    /// Optional topology policy for role dispatch.
    #[serde(default)]
    pub topology: Option<TopologySpec>,
    /// Optional supervisor escalation settings.
    #[serde(default)]
    pub supervisor: Option<SupervisorSpec>,
    /// Optional runtime limits for flows.
    #[serde(default)]
    pub limits: Option<LimitsSpec>,
    /// Optional declarative spawn policy for automatic member provisioning.
    #[serde(default)]
    pub spawn_policy: Option<SpawnPolicyConfig>,
    /// Optional declarative event router configuration.
    #[serde(default)]
    pub event_router: Option<EventRouterConfig>,
    /// If set, this mob is indexed to the given session.
    /// Used for lookup, resume plumbing, and host-side bookkeeping only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_id: Option<String>,
    /// Canonical cleanup policy for session-indexed mobs.
    #[serde(default, skip_serializing_if = "SessionCleanupPolicy::is_manual")]
    pub session_cleanup_policy: SessionCleanupPolicy,
    /// Whether this is an implicit delegation mob (created by `delegate` tool).
    /// Implicit mobs cannot be destroyed directly — they are cleaned up when
    /// the owning session is archived. Explicit mobs (created by `mob_create`)
    /// can be destroyed by their owning session.
    #[serde(default)]
    pub is_implicit: bool,
}

/// Helper struct for TOML deserialization of the `[mob]` section.
#[derive(Deserialize)]
struct TomlMob {
    id: MobId,
    orchestrator: Option<TomlOrchestrator>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TomlOrchestrator {
    Profile(String),
    Config(OrchestratorConfig),
}

/// Top-level TOML structure for mob definition files.
#[derive(Deserialize)]
struct TomlDefinition {
    mob: TomlMob,
    #[serde(default)]
    profiles: BTreeMap<ProfileName, ProfileBinding>,
    #[serde(default)]
    mcp: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    wiring: WiringRules,
    #[serde(default)]
    skills: BTreeMap<String, SkillSource>,
    #[serde(default)]
    backend: BackendConfig,
    #[serde(default)]
    flows: BTreeMap<FlowId, FlowSpec>,
    #[serde(default)]
    topology: Option<TopologySpec>,
    #[serde(default)]
    supervisor: Option<SupervisorSpec>,
    #[serde(default)]
    limits: Option<LimitsSpec>,
    #[serde(default)]
    spawn_policy: Option<SpawnPolicyConfig>,
    #[serde(default)]
    event_router: Option<EventRouterConfig>,
}

impl MobDefinition {
    /// Create a minimal implicit delegation mob indexed to the given session.
    ///
    /// The mob is tagged with `owner_session_id` for session-indexed lookup
    /// and marked session-scoped for cleanup. It also has
    /// `auto_wire_orchestrator = true` so spawned members are automatically
    /// wired to the lead agent.
    pub fn implicit(session_id: &str, model: &str) -> Self {
        let mob_id = MobId::from(format!("implicit-{session_id}"));
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("delegate"),
            ProfileBinding::Inline(Profile {
                model: model.to_string(),
                skills: Vec::new(),
                tools: crate::profile::ToolConfig {
                    comms: true,
                    ..crate::profile::ToolConfig::default()
                },
                peer_description: "Delegated sub-agent".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        let mut definition = Self {
            id: mob_id,
            orchestrator: None,
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules {
                auto_wire_orchestrator: true,
                role_wiring: Vec::new(),
            },
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_session_id: None,
            session_cleanup_policy: SessionCleanupPolicy::Manual,
            is_implicit: true,
        };
        definition.mark_session_scoped(session_id);
        definition
    }

    /// Parse a mob definition from TOML content.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        let raw: TomlDefinition = toml::from_str(content)?;
        let orchestrator = raw.mob.orchestrator.map(|orchestrator| match orchestrator {
            TomlOrchestrator::Profile(profile) => OrchestratorConfig {
                profile: ProfileName::from(profile),
            },
            TomlOrchestrator::Config(config) => config,
        });
        Ok(Self {
            id: raw.mob.id,
            orchestrator,
            profiles: raw.profiles,
            mcp_servers: raw.mcp,
            wiring: raw.wiring,
            skills: raw.skills,
            backend: raw.backend,
            flows: raw.flows,
            topology: raw.topology,
            supervisor: raw.supervisor,
            limits: raw.limits,
            spawn_policy: raw.spawn_policy,
            event_router: raw.event_router,
            owner_session_id: None,
            session_cleanup_policy: SessionCleanupPolicy::Manual,
            is_implicit: false,
        })
    }

    pub fn is_owned_by_session(&self, session_id: &str) -> bool {
        self.owner_session_id.as_deref() == Some(session_id)
    }

    pub fn is_session_scoped_to(&self, session_id: &str) -> bool {
        self.is_owned_by_session(session_id)
            && self.session_cleanup_policy == SessionCleanupPolicy::DestroyOnOwnerArchive
    }

    pub fn mark_session_scoped(&mut self, session_id: &str) {
        self.owner_session_id = Some(session_id.to_string());
        self.session_cleanup_policy = SessionCleanupPolicy::DestroyOnOwnerArchive;
    }

    pub fn clear_internal_lifecycle_flags(&mut self) {
        self.is_implicit = false;
        self.session_cleanup_policy = SessionCleanupPolicy::Manual;
    }

    /// Resolve an inline profile by name.
    ///
    /// Returns `Some(&Profile)` for `Inline` bindings, `None` for `RealmRef`
    /// bindings (which require async store lookup) or missing names.
    pub fn resolve_inline_profile(&self, name: &crate::ids::ProfileName) -> Option<&Profile> {
        self.profiles.get(name)?.as_inline()
    }

    /// Resolve a profile by name, supporting both inline and realm-ref bindings.
    ///
    /// For `Inline` bindings, returns the profile directly. For `RealmRef` bindings,
    /// looks up the profile from the provided realm profile store. Returns
    /// `MobError::ProfileNotFound` if the profile name is missing or the realm
    /// profile doesn't exist in the store, and `MobError::Internal` if a realm
    /// store is required but not available.
    pub async fn resolve_profile(
        &self,
        name: &crate::ids::ProfileName,
        realm_profile_store: Option<&std::sync::Arc<dyn crate::store::RealmProfileStore>>,
    ) -> Result<Profile, crate::error::MobError> {
        match self.profiles.get(name) {
            Some(ProfileBinding::Inline(p)) => Ok(p.clone()),
            Some(ProfileBinding::RealmRef { realm_profile }) => {
                let store = realm_profile_store.ok_or_else(|| {
                    crate::error::MobError::Internal(
                        "realm profile store not available for RealmRef resolution".into(),
                    )
                })?;
                store
                    .get(realm_profile)
                    .await
                    .map_err(crate::error::MobError::from)?
                    .ok_or_else(|| crate::error::MobError::ProfileNotFound(name.clone()))
                    .map(|stored| stored.profile)
            }
            None => Err(crate::error::MobError::ProfileNotFound(name.clone())),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]
mod tests {
    use super::*;
    use crate::profile::ToolConfig;

    fn example_toml() -> &'static str {
        r#"
[mob]
id = "code-review"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-6"
skills = ["orchestrator-skill"]
peer_description = "Coordinates code review"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.reviewer]
model = "claude-sonnet-4-5"
skills = ["reviewer-skill"]
peer_description = "Reviews code for quality"

[profiles.reviewer.tools]
builtins = true
shell = true
comms = true
mob_tasks = true
mcp = ["code-server"]

[mcp.code-server]
command = ["npx", "-y", "@mcp/code-server"]

[mcp.docs-server]
url = "https://docs.example.com/mcp"

[wiring]
auto_wire_orchestrator = true

[[wiring.role_wiring]]
a = "reviewer"
b = "reviewer"

[skills.orchestrator-skill]
source = "inline"
content = "You are the lead code reviewer."

[skills.reviewer-skill]
source = "path"
path = "skills/reviewer.md"
"#
    }

    #[test]
    fn test_mob_definition_from_toml() {
        let def = MobDefinition::from_toml(example_toml()).unwrap();
        assert_eq!(def.id.as_str(), "code-review");
        assert_eq!(def.orchestrator.as_ref().unwrap().profile.as_str(), "lead");
        assert_eq!(def.profiles.len(), 2);
        assert!(def.profiles.contains_key(&ProfileName::from("lead")));
        assert!(def.profiles.contains_key(&ProfileName::from("reviewer")));

        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        assert_eq!(lead.model, "claude-opus-4-6");
        assert!(lead.tools.mob);
        assert!(lead.tools.mob_tasks);
        assert!(lead.tools.comms);
        assert!(lead.external_addressable);

        let reviewer = def.profiles[&ProfileName::from("reviewer")]
            .as_inline()
            .unwrap();
        assert_eq!(reviewer.model, "claude-sonnet-4-5");
        assert!(reviewer.tools.shell);
        assert_eq!(reviewer.tools.mcp, vec!["code-server"]);

        assert_eq!(def.mcp_servers.len(), 2);
        assert!(def.mcp_servers.contains_key("code-server"));
        let code_server = &def.mcp_servers["code-server"];
        assert_eq!(
            code_server.command,
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "@mcp/code-server".to_string()
            ]
        );
        let docs_server = &def.mcp_servers["docs-server"];
        assert_eq!(
            docs_server.url.as_ref().unwrap(),
            "https://docs.example.com/mcp"
        );

        assert!(def.wiring.auto_wire_orchestrator);
        assert_eq!(def.wiring.role_wiring.len(), 1);
        assert_eq!(def.wiring.role_wiring[0].a.as_str(), "reviewer");
        assert_eq!(def.wiring.role_wiring[0].b.as_str(), "reviewer");

        assert_eq!(def.skills.len(), 2);
        match &def.skills["orchestrator-skill"] {
            SkillSource::Inline { content } => {
                assert_eq!(content, "You are the lead code reviewer.");
            }
            _ => panic!("expected inline skill"),
        }
        match &def.skills["reviewer-skill"] {
            SkillSource::Path { path } => {
                assert_eq!(path, "skills/reviewer.md");
            }
            _ => panic!("expected path skill"),
        }
    }

    #[test]
    fn test_mob_definition_toml_roundtrip() {
        let def = MobDefinition::from_toml(example_toml()).unwrap();
        // Serialize to JSON (stable format for roundtrip)
        let json = serde_json::to_string(&def).unwrap();
        let parsed: MobDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn test_mob_definition_json_roundtrip() {
        let def = MobDefinition {
            id: MobId::from("test-mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles: {
                let mut m = BTreeMap::new();
                m.insert(
                    ProfileName::from("lead"),
                    ProfileBinding::Inline(Profile {
                        model: "claude-opus-4-6".to_string(),
                        skills: vec!["skill-a".to_string()],
                        tools: ToolConfig::default(),
                        peer_description: "The leader".to_string(),
                        external_addressable: true,
                        backend: None,
                        runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                        max_inline_peer_notifications: None,
                        output_schema: None,
                        provider_params: None,
                    }),
                );
                m
            },
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_session_id: None,
            session_cleanup_policy: SessionCleanupPolicy::Manual,
            is_implicit: false,
        };
        let json = serde_json::to_string_pretty(&def).unwrap();
        let parsed: MobDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn test_minimal_toml() {
        let toml_str = r#"
[mob]
id = "minimal"
"#;
        let def = MobDefinition::from_toml(toml_str).unwrap();
        assert_eq!(def.id.as_str(), "minimal");
        assert!(def.orchestrator.is_none());
        assert!(def.profiles.is_empty());
        assert!(def.mcp_servers.is_empty());
        assert!(!def.wiring.auto_wire_orchestrator);
        assert!(def.wiring.role_wiring.is_empty());
        assert!(def.skills.is_empty());
        assert_eq!(def.backend.default, MobBackendKind::Session);
        assert!(def.backend.external.is_none());
        assert!(def.flows.is_empty());
        assert!(def.topology.is_none());
        assert!(def.supervisor.is_none());
        assert!(def.limits.is_none());
    }

    #[test]
    fn test_wiring_rules_serde_roundtrip() {
        let rules = WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: vec![RoleWiringRule {
                a: ProfileName::from("worker"),
                b: ProfileName::from("reviewer"),
            }],
        };
        let json = serde_json::to_string(&rules).unwrap();
        let parsed: WiringRules = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, rules);
    }

    #[test]
    fn test_mcp_server_config_serde() {
        let stdio = McpServerConfig {
            command: vec!["node".to_string(), "server.js".to_string()],
            url: None,
            env: {
                let mut m = BTreeMap::new();
                m.insert("API_KEY".to_string(), "secret".to_string());
                m
            },
        };
        let json = serde_json::to_string(&stdio).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, stdio);
    }

    #[test]
    fn test_mob_definition_from_toml_supports_orchestrator_table() {
        let toml = r#"
[mob]
id = "table-orchestrator"
orchestrator = { profile = "lead" }
"#;
        let def = MobDefinition::from_toml(toml).unwrap();
        assert_eq!(
            def.orchestrator.as_ref().map(|o| o.profile.as_str()),
            Some("lead")
        );
    }

    #[test]
    fn test_skill_source_serde() {
        let inline = SkillSource::Inline {
            content: "You are a helper.".to_string(),
        };
        let json = serde_json::to_string(&inline).unwrap();
        let parsed: SkillSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, inline);

        let path = SkillSource::Path {
            path: "skills/helper.md".to_string(),
        };
        let json = serde_json::to_string(&path).unwrap();
        let parsed: SkillSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, path);
    }

    #[test]
    fn test_flow_spec_toml_parse_preserves_author_order() {
        let toml = r#"
[mob]
id = "flow-mob"

[profiles.lead]
model = "claude-sonnet-4-5"

[flows.demo]
description = "demo"

[flows.demo.steps.first]
role = "lead"
message = "first"

[flows.demo.steps.second]
role = "lead"
message = "second"
        "#;
        let definition = MobDefinition::from_toml(toml).unwrap();
        let flow = definition
            .flows
            .get(&FlowId::from("demo"))
            .expect("flow exists");
        let step_order = flow.steps.keys().cloned().collect::<Vec<_>>();
        let step_order = step_order
            .into_iter()
            .map(|step_id| step_id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(step_order, vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn test_flow_and_topology_roundtrip() {
        let toml = r#"
[mob]
id = "flow-mob"

[profiles.lead]
model = "claude-sonnet-4-5"

[profiles.worker]
model = "claude-sonnet-4-5"

[flows.pipeline]
description = "pipeline flow"

[flows.pipeline.steps.start]
role = "lead"
message = "go"
dispatch_mode = "one_to_one"
depends_on_mode = "all"

[flows.pipeline.steps.branch_a]
role = "worker"
message = "a"
depends_on = ["start"]
branch = "choose"
condition = { op = "eq", path = "params.choice", value = "a" }

[flows.pipeline.steps.branch_b]
role = "worker"
message = "b"
depends_on = ["start"]
branch = "choose"
condition = { op = "eq", path = "params.choice", value = "b" }

[flows.pipeline.steps.join]
role = "lead"
message = "join"
depends_on = ["branch_a", "branch_b"]
depends_on_mode = "any"
collection_policy = { type = "quorum", n = 1 }
timeout_ms = 1000
expected_schema_ref = "schemas/join.json"

[topology]
mode = "strict"
rules = [{ from_role = "lead", to_role = "worker", allowed = true }]

[supervisor]
role = "lead"
escalation_threshold = 2

[limits]
max_flow_duration_ms = 30000
max_step_retries = 1
max_orphaned_turns = 8
        "#;

        let definition = MobDefinition::from_toml(toml).unwrap();
        assert!(definition.flows.contains_key(&FlowId::from("pipeline")));
        assert_eq!(
            definition.topology.as_ref().map(|t| t.mode.clone()),
            Some(PolicyMode::Strict)
        );
        assert_eq!(
            definition
                .supervisor
                .as_ref()
                .map(|s| s.escalation_threshold),
            Some(2)
        );
        assert_eq!(
            definition
                .limits
                .as_ref()
                .and_then(|l| l.max_orphaned_turns),
            Some(8)
        );

        let encoded = serde_json::to_string(&definition).unwrap();
        let decoded: MobDefinition = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, definition);
    }

    #[test]
    fn test_flow_step_output_format_defaults_to_json_and_parses_text() {
        let default_toml = r#"
[mob]
id = "flow-default-output"

[profiles.worker]
model = "claude-sonnet-4-5"

[flows.demo.steps.start]
role = "worker"
message = "hello"
        "#;
        let default_definition = MobDefinition::from_toml(default_toml).unwrap();
        let default_step = default_definition
            .flows
            .get(&FlowId::from("demo"))
            .and_then(|flow| flow.steps.get(&StepId::from("start")))
            .expect("step exists");
        assert_eq!(default_step.output_format, StepOutputFormat::Json);

        let text_toml = r#"
[mob]
id = "flow-text-output"

[profiles.worker]
model = "claude-sonnet-4-5"

[flows.demo.steps.start]
role = "worker"
message = "hello"
output_format = "text"
        "#;
        let text_definition = MobDefinition::from_toml(text_toml).unwrap();
        let text_step = text_definition
            .flows
            .get(&FlowId::from("demo"))
            .and_then(|flow| flow.steps.get(&StepId::from("start")))
            .expect("step exists");
        assert_eq!(text_step.output_format, StepOutputFormat::Text);
    }

    #[test]
    fn test_mob_definition_spawn_policy_auto_roundtrip() {
        let mut profile_map = BTreeMap::new();
        profile_map.insert("reviewer".to_string(), ProfileName::from("reviewer"));
        profile_map.insert("worker".to_string(), ProfileName::from("worker"));

        let policy = SpawnPolicyConfig::Auto {
            profile_map: profile_map.clone(),
        };
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: SpawnPolicyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
    }

    #[test]
    fn test_mob_definition_spawn_policy_none_roundtrip() {
        let policy = SpawnPolicyConfig::None;
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: SpawnPolicyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, policy);
    }

    #[test]
    fn test_mob_definition_spawn_policy_default_omitted() {
        let toml_str = r#"
[mob]
id = "no-spawn-policy"
"#;
        let def = MobDefinition::from_toml(toml_str).unwrap();
        assert!(def.spawn_policy.is_none());
    }

    #[test]
    fn test_mob_definition_event_router_roundtrip() {
        let config = EventRouterConfig {
            buffer_size: 512,
            include_patterns: Some(vec!["text_*".to_string()]),
            exclude_patterns: Some(vec!["debug_*".to_string()]),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: EventRouterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn test_mob_definition_event_router_defaults() {
        let json = r"{}";
        let parsed: EventRouterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.buffer_size, 256);
        assert!(parsed.include_patterns.is_none());
        assert!(parsed.exclude_patterns.is_none());
    }

    #[test]
    fn test_mob_definition_with_spawn_policy_and_event_router() {
        let toml_str = r#"
[mob]
id = "with-policy"

[spawn_policy]
mode = "auto"

[spawn_policy.profile_map]
reviewer = "reviewer"

[event_router]
buffer_size = 128
include_patterns = ["text_complete"]
"#;
        let def = MobDefinition::from_toml(toml_str).unwrap();
        assert!(def.spawn_policy.is_some());
        match &def.spawn_policy {
            Some(SpawnPolicyConfig::Auto { profile_map }) => {
                assert_eq!(
                    profile_map.get("reviewer"),
                    Some(&ProfileName::from("reviewer"))
                );
            }
            _ => panic!("expected Auto spawn policy"),
        }
        assert!(def.event_router.is_some());
        let router = def.event_router.as_ref().unwrap();
        assert_eq!(router.buffer_size, 128);
        assert_eq!(
            router.include_patterns,
            Some(vec!["text_complete".to_string()])
        );
    }

    #[test]
    fn test_frame_step_spec_roundtrip_json() {
        let spec = FrameStepSpec {
            step_id: StepId::from("step-a"),
            depends_on: vec![FlowNodeId::from("node-1")],
            depends_on_mode: DependencyMode::All,
            branch: None,
        };
        let encoded = serde_json::to_string(&spec).expect("serialize");
        let decoded: FrameStepSpec = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn test_repeat_until_spec_roundtrip_json() {
        let spec = RepeatUntilSpec {
            loop_id: LoopId::from("loop-a"),
            depends_on: vec![],
            depends_on_mode: DependencyMode::All,
            body: FrameSpec {
                nodes: indexmap::IndexMap::new(),
            },
            until: ConditionExpr::Eq {
                path: "steps.review.passed".into(),
                value: serde_json::json!(true),
            },
            max_iterations: 5,
        };
        let encoded = serde_json::to_string(&spec).expect("serialize");
        let decoded: RepeatUntilSpec = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn test_flow_node_spec_step_roundtrip_json() {
        let spec = FlowNodeSpec::Step(FrameStepSpec {
            step_id: StepId::from("step-b"),
            depends_on: vec![],
            depends_on_mode: DependencyMode::Any,
            branch: Some(BranchId::from("branch-1")),
        });
        let encoded = serde_json::to_string(&spec).expect("serialize");
        let decoded: FlowNodeSpec = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn test_frame_spec_roundtrip_json() {
        let mut nodes = indexmap::IndexMap::new();
        nodes.insert(
            FlowNodeId::from("node-a"),
            FlowNodeSpec::Step(FrameStepSpec {
                step_id: StepId::from("step-a"),
                depends_on: vec![],
                depends_on_mode: DependencyMode::All,
                branch: None,
            }),
        );
        let spec = FrameSpec { nodes };
        let encoded = serde_json::to_string(&spec).expect("serialize");
        let decoded: FrameSpec = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded.nodes.len(), 1);
    }

    #[test]
    fn test_flow_spec_with_root_roundtrip_json() {
        let spec = FlowSpec {
            description: Some("test flow".into()),
            steps: indexmap::IndexMap::new(),
            root: Some(FrameSpec {
                nodes: indexmap::IndexMap::new(),
            }),
        };
        let encoded = serde_json::to_string(&spec).expect("serialize");
        let decoded: FlowSpec = serde_json::from_str(&encoded).expect("deserialize");
        assert!(decoded.root.is_some());
    }

    #[test]
    fn test_flow_spec_without_root_deserializes_none() {
        // Legacy FlowSpec without root field deserializes with root: None
        let json = r#"{"description":null,"steps":{}}"#;
        let decoded: FlowSpec = serde_json::from_str(json).expect("deserialize");
        assert_eq!(decoded.root, None);
    }
}
