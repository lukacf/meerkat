//! Mob definition types and TOML parsing.
//!
//! A `MobDefinition` describes the complete structure of a mob: profiles,
//! wiring rules, and skill sources. Definitions are serializable so they can
//! be stored in `MobCreated` events for resume recovery.
//!
//! MCP servers are not a mob concept — members consume MCP tools from the
//! host's `McpRouterAdapter` (configured in `.rkat/mcp.toml`), and per-profile
//! scoping happens via `profile.tools.mcp` (an allowlist of host MCP source
//! IDs).

use crate::MobBackendKind;
use crate::ids::{BranchId, FlowId, FlowNodeId, LoopId, MobId, ProfileName, StepId};
use crate::profile::{Profile, ProfileBinding};
use indexmap::IndexMap;
use meerkat_core::schema::MeerkatSchema;
use meerkat_core::types::ContentInput;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Orchestrator configuration within a mob definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Profile name of the orchestrator.
    pub profile: ProfileName,
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
    /// Automatically wire every spawned member to the orchestrator.
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
    /// Supervisor bridge endpoint used by remote external members to send
    /// bridge replies back to this mob supervisor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_bridge: Option<SupervisorBridgeEndpointConfig>,
}

/// TCP endpoint configuration for the mob supervisor bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SupervisorBridgeEndpointConfig {
    /// Local socket address the supervisor bridge should bind, for example
    /// `0.0.0.0:42000` or `127.0.0.1:0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,
    /// Address advertised to external members, for example
    /// `tcp://supervisor.example.com:42000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advertised_address: Option<String>,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSpec {
    pub nodes: IndexMap<FlowNodeId, FlowNodeSpec>,
}

impl FrameSpec {
    /// Compile flat-authored steps into the canonical root frame.
    ///
    /// Flat `steps` remain a valid authoring ergonomic only because they are
    /// compiled into the execution root exactly once, at the decode/construct
    /// boundary; the runtime sees a single structure owner.
    #[must_use]
    pub fn from_flat_steps(steps: &IndexMap<StepId, FlowStepSpec>) -> Self {
        let nodes = steps
            .iter()
            .map(|(step_id, step)| {
                (
                    FlowNodeId::from(step_id.as_str()),
                    FlowNodeSpec::Step(FrameStepSpec {
                        step_id: step_id.clone(),
                        depends_on: step
                            .depends_on
                            .iter()
                            .map(|dependency| FlowNodeId::from(dependency.as_str()))
                            .collect(),
                        depends_on_mode: step.depends_on_mode.clone(),
                        branch: step.branch.clone(),
                    }),
                )
            })
            .collect();
        Self { nodes }
    }
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

/// Named (non-inline) reference to a step output schema.
///
/// The runtime resolves a `Named` ref against the host environment (today: a
/// filesystem path read at execution time). The newtype keeps the resolution
/// vocabulary typed and prevents accidental mixing with arbitrary strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaName(String);

impl SchemaName {
    /// Borrow the raw reference name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SchemaName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SchemaName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for SchemaName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Failure parsing an `expected_schema_ref` string into a typed [`FlowSchemaRef`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowSchemaRefParseError {
    /// The ref is empty, which is never a valid inline schema or named ref.
    #[error("expected_schema_ref must not be empty")]
    Empty,
    /// The ref looked like inline JSON but was not a valid Meerkat schema.
    #[error("inline schema is invalid: {message}")]
    InvalidInlineSchema {
        /// Display text of the underlying [`meerkat_core::schema::SchemaError`].
        message: String,
    },
}

/// Typed reference to the schema used to validate a step's structured output.
///
/// Parsed once at flow-definition load (parse-at-boundary): a ref that parses
/// as a JSON object is an [`FlowSchemaRef::Inline`] schema; any other non-empty
/// ref is a [`FlowSchemaRef::Named`] reference resolved by the runtime. The type
/// serializes transparently to a string so the persisted `FlowSpec`/
/// `MobDefinition` payload and the `MobFlowStepInput` wire shape remain a plain
/// `string`. `Named` refs round-trip byte-identically; `Inline` schemas
/// round-trip to their normalized canonical JSON-string form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowSchemaRef {
    /// An inline Meerkat schema parsed from JSON at the boundary.
    Inline(MeerkatSchema),
    /// A named reference (e.g. a filesystem path) resolved by the runtime.
    Named(SchemaName),
}

impl FlowSchemaRef {
    /// Parse a raw `expected_schema_ref` string into a typed ref.
    ///
    /// A string that parses as a JSON object becomes [`FlowSchemaRef::Inline`];
    /// any other non-empty string is treated as a [`FlowSchemaRef::Named`] ref.
    pub fn parse(raw: &str) -> Result<Self, FlowSchemaRefParseError> {
        if raw.trim().is_empty() {
            return Err(FlowSchemaRefParseError::Empty);
        }
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(value) if value.is_object() => {
                let schema = MeerkatSchema::new(value).map_err(|error| {
                    FlowSchemaRefParseError::InvalidInlineSchema {
                        message: error.to_string(),
                    }
                })?;
                Ok(Self::Inline(schema))
            }
            _ => Ok(Self::Named(SchemaName::from(raw))),
        }
    }

    /// Render the ref back to its raw string form (inverse of [`FlowSchemaRef::parse`]).
    pub fn as_raw(&self) -> String {
        match self {
            Self::Inline(schema) => schema.as_value().to_string(),
            Self::Named(name) => name.as_str().to_string(),
        }
    }
}

impl FromStr for FlowSchemaRef {
    type Err = FlowSchemaRefParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl Serialize for FlowSchemaRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_raw())
    }
}

impl<'de> Deserialize<'de> for FlowSchemaRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
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
    pub expected_schema_ref: Option<FlowSchemaRef>,
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct FlowSpec {
    pub description: Option<String>,
    pub steps: IndexMap<StepId, FlowStepSpec>,
    /// Canonical execution root. Always present: flat-authored specs are
    /// compiled into a root frame once at the decode/construct boundary.
    pub root: FrameSpec,
}

impl FlowSpec {
    /// Canonicalizing constructor.
    ///
    /// When `root` is omitted, the flat `steps` are compiled into the
    /// canonical root frame here — the single boundary where flat authoring
    /// becomes execution structure.
    #[must_use]
    pub fn new(
        description: Option<String>,
        steps: IndexMap<StepId, FlowStepSpec>,
        root: Option<FrameSpec>,
    ) -> Self {
        let root = root.unwrap_or_else(|| FrameSpec::from_flat_steps(&steps));
        Self {
            description,
            steps,
            root,
        }
    }
}

#[derive(Deserialize)]
struct FlowSpecDe {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    steps: IndexMap<StepId, FlowStepSpec>,
    #[serde(default)]
    root: Option<FrameSpec>,
}

impl<'de> Deserialize<'de> for FlowSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let de = FlowSpecDe::deserialize(deserializer)?;
        Ok(Self::new(de.description, de.steps, de.root))
    }
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
    /// Escalation turn timeout in milliseconds. Declared flow policy — the
    /// typed owner of the escalation deadline — rather than a hard-coded
    /// runtime constant. Absent means the runtime default applies.
    #[serde(default)]
    pub escalation_turn_timeout_ms: Option<u64>,
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

/// Complete mob definition.
///
/// Describes profiles, MCP servers, wiring rules, and skill sources.
/// Serializable for storage in `MobCreated` events. `rust_bundles` in
/// `ToolConfig` are stored as string names only; actual dispatchers
/// must be re-registered on resume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MobDefinition {
    /// Unique mob identifier.
    pub id: MobId,
    /// Optional orchestrator configuration.
    #[serde(default)]
    pub orchestrator: Option<OrchestratorConfig>,
    /// Named profiles for spawning mob members.
    ///
    /// Each profile can be an inline definition or a reference to a
    /// realm-scoped reusable profile.
    #[serde(default)]
    pub profiles: BTreeMap<ProfileName, ProfileBinding>,
    /// Mob-scoped custom model registry entries (`[models.<id>]`).
    ///
    /// Reuses the typed config owner [`meerkat_core::config::CustomModelConfig`]:
    /// one definition feeds provider inference, compaction scaling, capability
    /// gates, and call timeouts through the effective model registry at member
    /// build time.
    #[serde(default)]
    pub models: BTreeMap<String, meerkat_core::config::CustomModelConfig>,
    /// Mob-level default provider for `Auto` image-generation targets.
    ///
    /// Profiles may override per-profile via
    /// `Profile::image_generation_provider`.
    #[serde(default)]
    pub image_generation_provider: Option<meerkat_core::Provider>,
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
    /// Runtime create/resume lowers this static config into MobMachine before
    /// it can affect unknown-member admission.
    #[serde(default)]
    pub spawn_policy: Option<SpawnPolicyConfig>,
    /// Optional declarative event router configuration.
    #[serde(default)]
    pub event_router: Option<EventRouterConfig>,
}

impl Eq for MobDefinition {}

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
    models: BTreeMap<String, meerkat_core::config::CustomModelConfig>,
    #[serde(default)]
    image_generation_provider: Option<meerkat_core::Provider>,
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
    /// Create a minimal explicit mob definition with manual cleanup semantics.
    pub fn explicit(id: impl Into<MobId>) -> Self {
        Self {
            id: id.into(),
            orchestrator: None,
            profiles: BTreeMap::new(),
            models: BTreeMap::new(),
            image_generation_provider: None,
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
        }
    }

    /// Create a minimal implicit delegation mob request for the given bridge session.
    ///
    /// The bridge session parameter is retained for stable implicit mob id
    /// derivation. Runtime create lowers the owner-session binding and cleanup
    /// classification through generated `MobMachine` authority; this definition
    /// does not own those facts.
    /// The owning session is wired as an external peer by the delegate tool;
    /// implicit mobs do not create a local orchestrator member.
    #[doc(hidden)]
    pub fn implicit(bridge_session_id: &str, model: &str) -> Self {
        let mob_id = MobId::from(format!("implicit-{bridge_session_id}"));
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("delegate"),
            ProfileBinding::Inline(Profile {
                model: model.to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
        Self {
            id: mob_id,
            orchestrator: None,
            profiles,
            models: BTreeMap::new(),
            image_generation_provider: None,
            wiring: WiringRules {
                auto_wire_orchestrator: false,
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
        }
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
            models: raw.models,
            image_generation_provider: raw.image_generation_provider,
            wiring: raw.wiring,
            skills: raw.skills,
            backend: raw.backend,
            flows: raw.flows,
            topology: raw.topology,
            supervisor: raw.supervisor,
            limits: raw.limits,
            spawn_policy: raw.spawn_policy,
            event_router: raw.event_router,
        })
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
model = "claude-opus-4-8"
skills = ["orchestrator-skill"]
peer_description = "Coordinates code review"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true

[profiles.reviewer]
model = "claude-sonnet-4-5"
skills = ["reviewer-skill"]
peer_description = "Reviews code for quality"

[profiles.reviewer.tools]
builtins = true
shell = true
comms = true
mcp = ["code-server"]

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
    fn test_mob_definition_from_toml_parses_models_and_image_provider() {
        // `image_generation_provider` is a top-level definition fact (the
        // `[mob]` table only owns id/orchestrator), so it precedes the tables.
        let toml_str = r#"
image_generation_provider = "gemini"

[mob]
id = "custom-models"

[models.claude-internal-preview]
provider = "anthropic"
display_name = "Claude Internal Preview"
context_window = 500000
max_output_tokens = 16384
vision = true
call_timeout_secs = 900

[profiles.worker]
model = "claude-internal-preview"

[profiles.worker.tools]
comms = true
"#;
        let def = MobDefinition::from_toml(toml_str).unwrap();
        assert_eq!(
            def.image_generation_provider,
            Some(meerkat_core::Provider::Gemini)
        );
        let model = def
            .models
            .get("claude-internal-preview")
            .expect("custom model entry parses");
        assert_eq!(model.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(model.context_window, Some(500_000));
        assert_eq!(model.max_output_tokens, Some(16_384));
        assert_eq!(model.vision, Some(true));
        assert_eq!(model.web_search, None);
        assert_eq!(model.call_timeout_secs, Some(900));

        // Round-trips through serde (definitions are stored in MobCreated events).
        let json = serde_json::to_string(&def).unwrap();
        let parsed: MobDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn test_mob_definition_custom_model_provider_is_fail_closed() {
        let toml_str = r#"
[mob]
id = "custom-models"

[models.mystery-model]
provider = "mystery"

[profiles.worker]
model = "mystery-model"

[profiles.worker.tools]
comms = true
"#;
        assert!(
            MobDefinition::from_toml(toml_str).is_err(),
            "unknown custom-model provider names must fail closed at mob load"
        );
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
        assert_eq!(lead.model, "claude-opus-4-8");
        assert!(lead.tools.mob);
        assert!(lead.tools.comms);
        assert!(lead.external_addressable);

        let reviewer = def.profiles[&ProfileName::from("reviewer")]
            .as_inline()
            .unwrap();
        assert_eq!(reviewer.model, "claude-sonnet-4-5");
        assert!(reviewer.tools.shell);
        assert_eq!(reviewer.tools.mcp, vec!["code-server"]);

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
            models: BTreeMap::new(),
            image_generation_provider: None,
            profiles: {
                let mut m = BTreeMap::new();
                m.insert(
                    ProfileName::from("lead"),
                    ProfileBinding::Inline(Profile {
                        model: "claude-opus-4-8".to_string(),
                        provider: None,
                        self_hosted_server_id: None,
                        image_generation_provider: None,
                        auto_compact_threshold: None,
                        resume_overrides: Vec::new(),
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
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
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
    fn test_implicit_definition_does_not_mint_owner_cleanup_authority() {
        let def = MobDefinition::implicit("bridge-session", "gpt-5.4");
        let json = serde_json::to_value(&def).unwrap();
        assert!(json.get("owner_bridge_session_id").is_none());
        assert!(json.get("session_cleanup_policy").is_none());
        assert!(json.get("is_implicit").is_none());
        assert!(def.orchestrator.is_none());
        assert!(
            !def.wiring.auto_wire_orchestrator,
            "implicit delegate mobs rely on external owner wiring"
        );
    }

    #[test]
    fn test_legacy_lifecycle_projection_fields_are_deserialize_only_unknowns() {
        let base = MobDefinition::explicit("legacy-projection");
        let mut value = serde_json::to_value(&base).unwrap();
        value["owner_bridge_session_id"] =
            serde_json::json!("019dbd3d-d7ad-75a1-96d0-8013927e78f8");
        value["session_cleanup_policy"] = serde_json::json!("destroy_on_owner_archive");
        value["is_implicit"] = serde_json::json!(true);

        let parsed: MobDefinition = serde_json::from_value(value).unwrap();
        assert_eq!(
            parsed, base,
            "legacy lifecycle projections are ignored during definition decoding"
        );
        let serialized = serde_json::to_value(&parsed).unwrap();
        assert!(serialized.get("owner_bridge_session_id").is_none());
        assert!(serialized.get("session_cleanup_policy").is_none());
        assert!(serialized.get("is_implicit").is_none());
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
        let spec = FlowSpec::new(
            Some("test flow".into()),
            indexmap::IndexMap::new(),
            Some(FrameSpec {
                nodes: indexmap::IndexMap::new(),
            }),
        );
        let encoded = serde_json::to_string(&spec).expect("serialize");
        let decoded: FlowSpec = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded.root, spec.root);
    }

    #[test]
    fn test_flow_spec_without_root_synthesizes_root_from_flat_steps() {
        // Flat-authored specs are canonicalized at the decode boundary: the
        // omitted root is compiled from `steps` exactly once, here.
        let mut steps = indexmap::IndexMap::new();
        steps.insert(
            StepId::from("a"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: ContentInput::from("go".to_string()),
                depends_on: Vec::new(),
                dispatch_mode: DispatchMode::default(),
                collection_policy: CollectionPolicy::default(),
                condition: None,
                timeout_ms: None,
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: DependencyMode::default(),
                allowed_tools: None,
                blocked_tools: None,
                output_format: StepOutputFormat::default(),
            },
        );
        let mut value =
            serde_json::to_value(FlowSpec::new(None, steps.clone(), None)).expect("serialize");
        value
            .as_object_mut()
            .expect("flow spec serializes as object")
            .remove("root");
        let decoded: FlowSpec = serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded.root, FrameSpec::from_flat_steps(&steps));
        assert!(decoded.root.nodes.contains_key(&FlowNodeId::from("a")));
    }

    #[test]
    fn test_flow_schema_ref_parses_named_from_path() {
        let parsed = FlowSchemaRef::parse("schemas/join.json").expect("named ref");
        assert_eq!(
            parsed,
            FlowSchemaRef::Named(SchemaName::from("schemas/join.json"))
        );
        assert_eq!(parsed.as_raw(), "schemas/join.json");
    }

    #[test]
    fn test_flow_schema_ref_parses_inline_json_object() {
        let raw = r#"{"type":"object"}"#;
        let parsed = FlowSchemaRef::parse(raw).expect("inline ref");
        assert!(matches!(parsed, FlowSchemaRef::Inline(_)));
    }

    #[test]
    fn test_flow_schema_ref_rejects_empty() {
        assert_eq!(
            FlowSchemaRef::parse("   "),
            Err(FlowSchemaRefParseError::Empty)
        );
    }

    #[test]
    fn test_flow_schema_ref_named_serializes_transparently_as_string() {
        let step_json = serde_json::json!({
            "role": "worker",
            "message": "go",
            "expected_schema_ref": "schemas/join.json"
        });
        let step: FlowStepSpec = serde_json::from_value(step_json).expect("decode step");
        assert_eq!(
            step.expected_schema_ref,
            Some(FlowSchemaRef::Named(SchemaName::from("schemas/join.json")))
        );
        let reencoded = serde_json::to_value(&step).expect("encode step");
        assert_eq!(reencoded["expected_schema_ref"], "schemas/join.json");
    }

    #[test]
    fn test_flow_schema_ref_inline_roundtrips_as_string() {
        let inline = FlowSchemaRef::parse(r#"{"type":"object"}"#).expect("inline ref");
        let encoded = serde_json::to_value(&inline).expect("encode");
        // Inline schema serializes back to its JSON-string form, parseable again.
        let decoded: FlowSchemaRef = serde_json::from_value(encoded).expect("decode");
        assert!(matches!(decoded, FlowSchemaRef::Inline(_)));
    }

    #[test]
    fn test_flow_and_topology_roundtrip_preserves_named_schema_ref() {
        let definition = MobDefinition::from_toml(
            r#"
[mob]
id = "schema-mob"

[profiles.lead]
model = "claude-sonnet-4-5"

[flows.demo.steps.join]
role = "lead"
message = "join"
expected_schema_ref = "schemas/join.json"
            "#,
        )
        .unwrap();
        let step = definition
            .flows
            .get(&FlowId::from("demo"))
            .and_then(|flow| flow.steps.get(&StepId::from("join")))
            .expect("join step exists");
        assert_eq!(
            step.expected_schema_ref,
            Some(FlowSchemaRef::Named(SchemaName::from("schemas/join.json")))
        );

        let encoded = serde_json::to_string(&definition).unwrap();
        let decoded: MobDefinition = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, definition);
    }
}
