//! Pack definitions — reusable team compositions for the `deliberate` tool.
//!
//! Each pack implements the [`Pack`] trait and builds a [`MobDefinition`] that
//! the deliberate handler uses to create an ephemeral mob. Packs come in two
//! execution modes:
//!
//! - **Flow-based** (`flow_step_count() > 0`): Structured step execution with
//!   dependency tracking. The deliberate handler runs the `"main"` flow and
//!   polls for completion. Prior step outputs are forwarded via `{{ steps.<id> }}`
//!   template references.
//!
//! - **Comms-based** (`flow_step_count() == 0`): Autonomous agents communicate
//!   freely via peer messaging. The deliberate handler injects the task via
//!   `mob_send_message` and monitors the event stream for quiescence.

pub mod advisor;
pub mod architect;
pub mod brainstorm;
pub mod panel;
pub mod rct;
pub mod red_team;
pub mod review;

use std::collections::BTreeMap;

use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;
use serde_json::Value;

// ── Pack trait ───────────────────────────────────────────────────────────────

/// A named pack that builds a [`MobDefinition`] for a specific collaboration pattern.
pub trait Pack: Send + Sync {
    /// Stable name used in the `deliberate` tool's `pack` parameter.
    fn name(&self) -> &str;
    /// Human-readable description shown in `list_packs`.
    fn description(&self) -> &str;
    /// Number of agents this pack spawns.
    fn agent_count(&self) -> usize;
    /// Number of flow steps (0 = comms-based, no flow).
    fn flow_step_count(&self) -> usize;
    /// Build the [`MobDefinition`] with task/context interpolated and overrides applied.
    fn definition(
        &self,
        task: &str,
        context: &str,
        model_overrides: &BTreeMap<String, String>,
        provider_params: Option<&Value>,
    ) -> MobDefinition;
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// Registry of all available packs.
pub struct PackRegistry {
    packs: BTreeMap<String, Box<dyn Pack>>,
}

impl PackRegistry {
    pub fn new() -> Self {
        let mut packs = BTreeMap::<String, Box<dyn Pack>>::new();
        packs.insert("advisor".into(), Box::new(advisor::AdvisorPack));
        packs.insert("review".into(), Box::new(review::ReviewPack));
        packs.insert("architect".into(), Box::new(architect::ArchitectPack));
        packs.insert("brainstorm".into(), Box::new(brainstorm::BrainstormPack));
        packs.insert("red-team".into(), Box::new(red_team::RedTeamPack));
        packs.insert("panel".into(), Box::new(panel::PanelPack));
        packs.insert("rct".into(), Box::new(rct::RctPack));
        Self { packs }
    }

    pub fn get(&self, name: &str) -> Option<&dyn Pack> {
        self.packs.get(name).map(|p| p.as_ref())
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.packs.keys().map(|s| s.as_str()).collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &dyn Pack> {
        self.packs.values().map(|p| p.as_ref())
    }
}

// ── Shared helpers (reduce boilerplate across pack definitions) ──────────────

/// Resolve a model name: use the override if provided, otherwise the default.
pub fn resolve_model(overrides: &BTreeMap<String, String>, role: &str, default: &str) -> String {
    overrides
        .get(role)
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

/// Format the context block for injection into flow step messages.
pub fn format_context(context: &str) -> String {
    if context.is_empty() {
        String::new()
    } else {
        format!("\n\n## Context\n\n{context}")
    }
}

/// Build a turn-driven profile (no tools except comms). Used by most flow-based packs.
pub fn turn_driven_profile(
    model: String,
    skill: &str,
    desc: &str,
    provider_params: Option<&Value>,
) -> Profile {
    Profile {
        model,
        skills: vec![skill.to_string()],
        tools: ToolConfig {
            comms: true,
            ..ToolConfig::default()
        },
        peer_description: desc.to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::TurnDriven,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: provider_params.cloned(),
    }
}

/// Build a flow step with text output mode and common defaults.
pub fn flow_step(role: &str, message: String, depends_on: &[&str], timeout_ms: u64) -> FlowStepSpec {
    FlowStepSpec {
        role: ProfileName::from(role),
        message,
        depends_on: depends_on.iter().map(|s| StepId::from(*s)).collect(),
        dispatch_mode: DispatchMode::default(),
        collection_policy: CollectionPolicy::default(),
        condition: None,
        timeout_ms: Some(timeout_ms),
        expected_schema_ref: None,
        branch: None,
        depends_on_mode: DependencyMode::default(),
        allowed_tools: None,
        blocked_tools: None,
        output_format: StepOutputFormat::Text,
    }
}

/// Build an identity spawn policy (meerkat_id == profile name for each agent).
pub fn identity_spawn_policy(names: &[&str]) -> Option<SpawnPolicyConfig> {
    let profile_map = names
        .iter()
        .map(|n| (n.to_string(), ProfileName::from(*n)))
        .collect();
    Some(SpawnPolicyConfig::Auto { profile_map })
}

/// Build a MobDefinition with common defaults filled in.
pub fn mob_definition(
    id_prefix: &str,
    profiles: BTreeMap<ProfileName, Profile>,
    skills: BTreeMap<String, SkillSource>,
    flows: BTreeMap<FlowId, FlowSpec>,
    spawn_policy: Option<SpawnPolicyConfig>,
) -> MobDefinition {
    MobDefinition {
        id: MobId::from(format!("force-{id_prefix}-{}", uuid::Uuid::new_v4().as_simple())),
        orchestrator: None,
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules::default(),
        skills,
        backend: BackendConfig::default(),
        flows,
        topology: None,
        supervisor: None,
        limits: None,
        spawn_policy,
        event_router: None,
    }
}
