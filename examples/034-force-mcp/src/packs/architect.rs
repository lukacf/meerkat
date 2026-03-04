use std::collections::BTreeMap;

use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::{Pack, text_schema};

pub struct ArchitectPack;

impl Pack for ArchitectPack {
    fn name(&self) -> &str {
        "architect"
    }

    fn description(&self) -> &str {
        "Design deliberation: planner proposes, critic challenges, synthesizer produces ADR"
    }

    fn agent_count(&self) -> usize {
        3
    }

    fn flow_step_count(&self) -> usize {
        4
    }

    fn definition(
        &self,
        task: &str,
        context: &str,
        model_overrides: &BTreeMap<String, String>,
    ) -> MobDefinition {
        let ctx = if context.is_empty() {
            String::new()
        } else {
            format!("\n\n## Context\n\n{context}")
        };

        let model = |role: &str, default: &str| -> String {
            model_overrides
                .get(role)
                .cloned()
                .unwrap_or_else(|| default.to_string())
        };

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, m) in [
            ("planner", "planner-skill", "Architecture planner", model("planner", "claude-opus-4-6")),
            ("critic", "critic-skill", "Architecture critic", model("critic", "claude-sonnet-4-5")),
            ("synthesizer", "synthesizer-skill", "Decision record writer", model("synthesizer", "claude-sonnet-4-5")),
        ] {
            profiles.insert(
                ProfileName::from(name),
                Profile {
                    model: m,
                    skills: vec![skill.to_string()],
                    tools: ToolConfig { comms: true, ..ToolConfig::default() },
                    peer_description: desc.to_string(),
                    external_addressable: true,
                    backend: None,
                    runtime_mode: MobRuntimeMode::TurnDriven,
                    max_inline_peer_notifications: None,
                    output_schema: Some(text_schema()),
                    provider_params: None,
                },
            );
        }

        let mut skills = BTreeMap::new();
        skills.insert("planner-skill".into(), SkillSource::Inline { content: include_str!("../../skills/planner.md").into() });
        skills.insert("critic-skill".into(), SkillSource::Inline { content: include_str!("../../skills/critic.md").into() });
        skills.insert("synthesizer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/synthesizer.md").into() });

        let step = |role: &str, msg: String, deps: Vec<&str>| -> FlowStepSpec {
            FlowStepSpec {
                role: ProfileName::from(role),
                message: msg,
                depends_on: deps.into_iter().map(StepId::from).collect(),
                dispatch_mode: DispatchMode::default(),
                collection_policy: CollectionPolicy::default(),
                condition: None,
                timeout_ms: Some(180_000),
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: DependencyMode::default(),
                allowed_tools: None,
                blocked_tools: None,
            }
        };

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("plan"), step("planner", format!("Create an architecture plan for:\n\n{task}{ctx}"), vec![]));
        steps.insert(StepId::from("critique"), step("critic", "Critique this architecture plan. Find weaknesses, missing considerations, and risky assumptions.".into(), vec!["plan"]));
        steps.insert(StepId::from("revise"), step("planner", "Address the critique. Revise your plan to address the weaknesses identified.".into(), vec!["critique"]));
        steps.insert(StepId::from("synthesize"), step("synthesizer", "Produce a final Architecture Decision Record (ADR) with: context, decision, consequences, alternatives considered.".into(), vec!["revise"]));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Architecture deliberation".into()), steps });

        let mut profile_map = BTreeMap::new();
        for name in ["planner", "critic", "synthesizer"] {
            profile_map.insert(name.into(), ProfileName::from(name));
        }

        MobDefinition {
            id: MobId::from(format!("force-architect-{}", uuid::Uuid::new_v4().as_simple())),
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
            spawn_policy: Some(SpawnPolicyConfig::Auto { profile_map }),
            event_router: None,
        }
    }
}
