use std::collections::BTreeMap;

use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::{Pack, text_schema};

pub struct BrainstormPack;

impl Pack for BrainstormPack {
    fn name(&self) -> &str {
        "brainstorm"
    }

    fn description(&self) -> &str {
        "Multi-model ideation: 3 diverse perspectives + synthesis with ranked ideas"
    }

    fn agent_count(&self) -> usize {
        4
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

        // Intentionally diverse default models for different perspectives
        let agents = [
            ("ideator_a", "ideator-a-skill", "Practical ideator", model("ideator_a", "claude-sonnet-4-5")),
            ("ideator_b", "ideator-b-skill", "Creative ideator", model("ideator_b", "gpt-5.2")),
            ("ideator_c", "ideator-c-skill", "Contrarian ideator", model("ideator_c", "gemini-3-flash-preview")),
            ("synthesizer", "synthesizer-skill", "Idea synthesizer", model("synthesizer", "claude-sonnet-4-5")),
        ];

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, m) in &agents {
            profiles.insert(
                ProfileName::from(*name),
                Profile {
                    model: m.clone(),
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
        skills.insert("ideator-a-skill".into(), SkillSource::Inline { content: include_str!("../../skills/ideator_a.md").into() });
        skills.insert("ideator-b-skill".into(), SkillSource::Inline { content: include_str!("../../skills/ideator_b.md").into() });
        skills.insert("ideator-c-skill".into(), SkillSource::Inline { content: include_str!("../../skills/ideator_c.md").into() });
        skills.insert("synthesizer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/synthesizer.md").into() });

        let ideate_msg = format!("Generate ideas for:\n\n{task}{ctx}");

        let step = |role: &str, msg: String, deps: Vec<&str>| -> FlowStepSpec {
            FlowStepSpec {
                role: ProfileName::from(role),
                message: msg,
                depends_on: deps.into_iter().map(StepId::from).collect(),
                dispatch_mode: DispatchMode::default(),
                collection_policy: CollectionPolicy::default(),
                condition: None,
                timeout_ms: Some(120_000),
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: DependencyMode::default(),
                allowed_tools: None,
                blocked_tools: None,
            }
        };

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("ideate_a"), step("ideator_a", ideate_msg.clone(), vec![]));
        steps.insert(StepId::from("ideate_b"), step("ideator_b", ideate_msg.clone(), vec![]));
        steps.insert(StepId::from("ideate_c"), step("ideator_c", ideate_msg, vec![]));
        steps.insert(StepId::from("synthesize"), step(
            "synthesizer",
            "Synthesize all ideas below into a ranked list. For each idea: describe it, assess feasibility, note trade-offs. Recommend the top 2-3 approaches.\n\n## Practical Perspective\n{{ steps.ideate_a.response }}\n\n## Creative Perspective\n{{ steps.ideate_b.response }}\n\n## Contrarian Perspective\n{{ steps.ideate_c.response }}".into(),
            vec!["ideate_a", "ideate_b", "ideate_c"],
        ));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Multi-perspective brainstorm".into()), steps });

        let mut profile_map = BTreeMap::new();
        for (name, ..) in &agents {
            profile_map.insert(name.to_string(), ProfileName::from(*name));
        }

        MobDefinition {
            id: MobId::from(format!("force-brainstorm-{}", uuid::Uuid::new_v4().as_simple())),
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
