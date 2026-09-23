//! Pack definitions — reusable team compositions for the `deliberate` tool.
//!
//! Each pack implements the [`Pack`] trait and builds a [`MobDefinition`] that
//! the deliberate handler uses to create an ephemeral mob. Every pack must
//! define a `"main"` flow: structured step execution gives `MobMachine` the
//! terminal result, and prior step outputs are forwarded via
//! `{{ steps.<id> }}` template references.

pub mod advisor;
pub mod architect;
pub mod brainstorm;
pub mod implement;
pub mod panel;
pub mod rct;
pub mod red_team;
pub mod review;

use std::collections::BTreeMap;

use meerkat_core::types::ContentInput;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ProfileBinding, ToolConfig};
use meerkat_mob::MobRuntimeMode;

// ── Pack trait ───────────────────────────────────────────────────────────────

/// A named pack that builds a [`MobDefinition`] for a specific collaboration pattern.
pub trait Pack: Send + Sync {
    /// Stable name used in the `deliberate` tool's `pack` parameter.
    fn name(&self) -> &str;
    /// Human-readable description shown in `list_packs`.
    fn description(&self) -> &str;
    /// Number of agents this pack spawns.
    fn agent_count(&self) -> usize;
    /// Number of flow steps in the required `"main"` flow.
    fn flow_step_count(&self) -> usize;
    /// Build fixed flow templates. Caller text is supplied as activation parameters.
    fn definition(
        &self,
        model_overrides: &BTreeMap<String, String>,
        provider_params: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> MobDefinition;

    fn roles(&self) -> BTreeMap<String, String> {
        self.definition(&BTreeMap::new(), None)
            .profiles
            .iter()
            .filter_map(|(name, binding)| {
                binding
                    .as_inline()
                    .map(|profile| (name.to_string(), profile.model.clone()))
            })
            .collect()
    }
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
        packs.insert("implement".into(), Box::new(implement::ImplementPack));
        packs.insert("rct".into(), Box::new(rct::RctPack));
        Self { packs }
    }

    /// Register a dynamic pack (e.g. user-created). Overwrites if name exists.
    pub fn register(&mut self, pack: Box<dyn Pack>) {
        self.packs.insert(pack.name().to_string(), pack);
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

pub const TASK_TEMPLATE: &str = "{{ params.task }}{{ params.context }}";

pub fn flow_params(task: &str, context: &str) -> serde_json::Value {
    serde_json::json!({"task": task, "context": format_context(context)})
}

/// Build a turn-driven profile with file-reading builtins and comms.
/// Used by most flow-based packs so agents can read repository files
/// during code review, architecture, and deliberation tasks.
pub fn turn_driven_profile(
    model: String,
    skill: &str,
    desc: &str,
    provider_params: Option<&meerkat_core::ProviderParamsOverride>,
) -> ProfileBinding {
    ProfileBinding::Inline(Box::new(Profile {
        model,
        model_fallback: None,
        provider: None,
        self_hosted_server_id: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: vec![skill.to_string()],
        tools: ToolConfig {
            builtins: true,
            shell: true,
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
    }))
}

/// Build a flow step with text output mode and common defaults.
pub fn flow_step(
    role: &str,
    message: String,
    depends_on: &[&str],
    timeout_ms: u64,
) -> FlowStepSpec {
    FlowStepSpec {
        role: ProfileName::from(role),
        message: ContentInput::from(message),
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
        output_format: Some(StepOutputFormat::Text),
        failure_policy: Default::default(),
    }
}

/// Build an identity spawn policy (agent_identity == profile name for each agent).
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
    profiles: BTreeMap<ProfileName, ProfileBinding>,
    skills: BTreeMap<String, SkillSource>,
    flows: BTreeMap<FlowId, FlowSpec>,
    spawn_policy: Option<SpawnPolicyConfig>,
) -> MobDefinition {
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "codemob-{id_prefix}-{}",
        uuid::Uuid::new_v4().as_simple()
    )));
    definition.profiles = profiles;
    definition.skills = skills;
    definition.flows = flows;
    definition.spawn_policy = spawn_policy;
    definition
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_pack_defaults_preserve_intentional_model_diversity() {
        let registry = PackRegistry::new();
        let mut models = Vec::new();

        for pack in registry.all() {
            let definition = pack.definition(&BTreeMap::new(), None);
            for binding in definition.profiles.values() {
                let profile = binding
                    .as_inline()
                    .expect("built-in packs should use inline profiles");
                models.push(profile.model.clone());
            }
        }

        assert!(
            models.iter().any(|model| model == "gpt-5.5"),
            "the deliberately prior-generation OpenAI role should remain in the pack mix: {models:?}"
        );
        assert!(
            models.iter().any(|model| model == "claude-opus-4-8"),
            "the Anthropic role mix should include claude-opus-4-8: {models:?}"
        );
        assert!(
            !models.iter().any(|model| model == "gpt-5.2"),
            "advanced packs should not regress to gpt-5.2: {models:?}"
        );
        assert!(
            !models.iter().any(|model| model == "gpt-5.4"),
            "advanced packs should not regress to gpt-5.4: {models:?}"
        );
    }

    #[test]
    fn built_in_pack_defaults_keep_model_diversity_within_each_pack() {
        let registry = PackRegistry::new();

        for pack in registry.all() {
            let definition = pack.definition(&BTreeMap::new(), None);
            let mut seen = std::collections::BTreeSet::new();
            let mut models = Vec::new();

            for binding in definition.profiles.values() {
                let profile = binding
                    .as_inline()
                    .expect("built-in packs should use inline profiles");
                models.push(profile.model.clone());
                assert!(
                    seen.insert(profile.model.clone()),
                    "pack '{}' should not duplicate default model '{}'; models: {models:?}",
                    pack.name(),
                    profile.model
                );
            }
        }
    }

    #[test]
    fn built_in_packs_all_have_machine_owned_flows() {
        let registry = PackRegistry::new();

        for pack in registry.all() {
            let definition = pack.definition(&BTreeMap::new(), None);
            assert!(
                pack.flow_step_count() > 0,
                "pack '{}' must expose a machine-owned main flow",
                pack.name()
            );
            assert!(
                definition.flows.contains_key(&FlowId::from("main")),
                "pack '{}' must define a main flow",
                pack.name()
            );
        }
    }

    #[test]
    fn bounded_role_instructions_match_pack_graphs_and_role_inventory() {
        let registry = PackRegistry::new();
        let mut checked = 0;
        for (name, steps) in [("implement", 2), ("panel", 6), ("rct", 6)] {
            let pack = registry.get(name).unwrap();
            let definition = pack.definition(&BTreeMap::new(), None);
            let main = &definition.flows[&FlowId::from("main")];
            assert_eq!(main.steps.len(), steps);
            assert_eq!(pack.roles().len(), definition.profiles.len());
            for (role, model) in pack.roles() {
                assert_eq!(
                    definition.profiles[&ProfileName::from(role)]
                        .as_inline()
                        .unwrap()
                        .model,
                    model
                );
            }
            for (key, source) in &definition.skills {
                if matches!(
                    key.as_str(),
                    "implementer-skill"
                        | "moderator-skill"
                        | "rct-orchestrator-skill"
                        | "purist-skill"
                        | "pragmatist-skill"
                        | "skeptic-skill"
                        | "veteran-skill"
                ) {
                    checked += 1;
                    let SkillSource::Inline { content } = source else {
                        panic!("embedded role must be inline")
                    };
                    assert!(content.contains("flow"), "{key}");
                    assert!(!content.contains("Continue revising until"));
                    assert!(!content.contains("spawn implementer"));
                    assert!(!content.contains("After 3-4 exchanges"));
                }
            }
        }
        assert_eq!(checked, 7);
    }
}
