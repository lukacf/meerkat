use std::collections::BTreeMap;

use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::Pack;

pub struct RedTeamPack;

impl Pack for RedTeamPack {
    fn name(&self) -> &str {
        "red-team"
    }

    fn description(&self) -> &str {
        "Balanced risk assessment: advocate argues for, adversary argues against, judge decides"
    }

    fn agent_count(&self) -> usize {
        3
    }

    fn flow_step_count(&self) -> usize {
        3
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

        let agents = [
            ("advocate", "advocate-skill", "Advocate — argues in favor", model("advocate", "claude-sonnet-4-5")),
            ("adversary", "adversary-skill", "Adversary — argues against", model("adversary", "claude-sonnet-4-5")),
            ("judge", "judge-skill", "Judge — balanced assessment", model("judge", "claude-opus-4-6")),
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
                    output_schema: None,
                    provider_params: None,
                },
            );
        }

        let mut skills = BTreeMap::new();
        skills.insert("advocate-skill".into(), SkillSource::Inline { content: include_str!("../../skills/advocate.md").into() });
        skills.insert("adversary-skill".into(), SkillSource::Inline { content: include_str!("../../skills/adversary.md").into() });
        skills.insert("judge-skill".into(), SkillSource::Inline { content: include_str!("../../skills/judge.md").into() });

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
                blocked_tools: None, output_format: StepOutputFormat::Text,
            }
        };

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("advocate_case"), step("advocate", format!("Argue IN FAVOR of this approach. Build the strongest possible case:\n\n{task}{ctx}"), vec![]));
        steps.insert(StepId::from("adversary_case"), step("adversary", format!("Argue AGAINST this approach. Find every risk, cost, and failure mode:\n\n{task}{ctx}"), vec![]));
        steps.insert(StepId::from("judgment"), step("judge", "Weigh both arguments below and produce a balanced risk assessment with a clear recommendation.\n\n## Advocate (Pro)\n{{ steps.advocate_case }}\n\n## Adversary (Con)\n{{ steps.adversary_case }}".into(), vec!["advocate_case", "adversary_case"]));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Adversarial risk assessment".into()), steps });

        let mut profile_map = BTreeMap::new();
        for (name, ..) in &agents {
            profile_map.insert(name.to_string(), ProfileName::from(*name));
        }

        MobDefinition {
            id: MobId::from(format!("force-redteam-{}", uuid::Uuid::new_v4().as_simple())),
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
