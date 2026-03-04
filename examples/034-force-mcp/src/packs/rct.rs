use std::collections::BTreeMap;

use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::Pack;

pub struct RctPack;

impl Pack for RctPack {
    fn name(&self) -> &str {
        "rct"
    }

    fn description(&self) -> &str {
        "Full RCT pipeline: plan, implement, 3 parallel gate reviewers, aggregate verdict"
    }

    fn agent_count(&self) -> usize {
        6
    }

    fn flow_step_count(&self) -> usize {
        6
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

        let tools_with_shell = ToolConfig {
            builtins: true,
            shell: true,
            ..ToolConfig::default()
        };

        let agents: Vec<(&str, &str, &str, String, ToolConfig, MobRuntimeMode)> = vec![
            ("orchestrator", "rct-orchestrator-skill", "RCT pipeline orchestrator",
             model("orchestrator", "claude-opus-4-6"), tools_with_shell.clone(), MobRuntimeMode::TurnDriven),
            ("implementer", "rct-implementer-skill", "Implementation agent",
             model("implementer", "claude-sonnet-4-5"), tools_with_shell.clone(), MobRuntimeMode::TurnDriven),
            ("rct_guardian", "rct-guardian-skill", "RCT Guardian reviewer",
             model("rct_guardian", "claude-sonnet-4-5"), tools_with_shell.clone(), MobRuntimeMode::TurnDriven),
            ("integration_sheriff", "rct-sheriff-skill", "Integration Sheriff reviewer",
             model("integration_sheriff", "claude-sonnet-4-5"), tools_with_shell.clone(), MobRuntimeMode::TurnDriven),
            ("spec_auditor", "rct-auditor-skill", "Spec Auditor reviewer",
             model("spec_auditor", "claude-sonnet-4-5"), tools_with_shell.clone(), MobRuntimeMode::TurnDriven),
            ("aggregator", "rct-aggregator-skill", "Gate verdict aggregator",
             model("aggregator", "claude-sonnet-4-5"), ToolConfig::default(), MobRuntimeMode::TurnDriven),
        ];

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, m, tools, mode) in &agents {
            profiles.insert(
                ProfileName::from(*name),
                Profile {
                    model: m.clone(),
                    skills: vec![skill.to_string()],
                    tools: tools.clone(),
                    peer_description: desc.to_string(),
                    external_addressable: true,
                    backend: None,
                    runtime_mode: *mode,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                },
            );
        }

        let mut skills = BTreeMap::new();
        skills.insert("rct-orchestrator-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_orchestrator.md").into() });
        skills.insert("rct-implementer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_implementer.md").into() });
        skills.insert("rct-guardian-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_guardian.md").into() });
        skills.insert("rct-sheriff-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_integration_sheriff.md").into() });
        skills.insert("rct-auditor-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_spec_auditor.md").into() });
        skills.insert("rct-aggregator-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_aggregator.md").into() });

        let step = |role: &str, msg: String, deps: Vec<&str>, timeout_ms: u64| -> FlowStepSpec {
            FlowStepSpec {
                role: ProfileName::from(role),
                message: msg,
                depends_on: deps.into_iter().map(StepId::from).collect(),
                dispatch_mode: DispatchMode::default(),
                collection_policy: CollectionPolicy::default(),
                condition: None,
                timeout_ms: Some(timeout_ms),
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: DependencyMode::default(),
                allowed_tools: None,
                blocked_tools: None,
            }
        };

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("plan"), step(
            "orchestrator",
            format!("Analyze this task and create an implementation plan. Create .rct/spec.yaml with requirements and .rct/checklist.yaml with phased tasks.\n\n{task}{ctx}"),
            vec![],
            300_000, // 5 min
        ));
        steps.insert(StepId::from("implement"), step(
            "implementer",
            "Implement the plan created by the orchestrator. Read .rct/checklist.yaml for tasks. Run verification commands after each task. Mark tasks as done.".into(),
            vec!["plan"],
            600_000, // 10 min
        ));
        // 3 reviewers run in parallel (no deps between them, all depend on implement)
        steps.insert(StepId::from("review_rct"), step(
            "rct_guardian",
            "Gate review: Check representation contracts. Run tests independently. Produce verdict.".into(),
            vec!["implement"],
            180_000, // 3 min
        ));
        steps.insert(StepId::from("review_integration"), step(
            "integration_sheriff",
            "Gate review: Check cross-component wiring. Run integration tests independently. Produce verdict.".into(),
            vec!["implement"],
            180_000,
        ));
        steps.insert(StepId::from("review_spec"), step(
            "spec_auditor",
            "Gate review: Check requirements compliance against .rct/spec.yaml. Produce verdict.".into(),
            vec!["implement"],
            180_000,
        ));
        steps.insert(StepId::from("aggregate"), step(
            "aggregator",
            "Aggregate the 3 reviewer verdicts into a final gate result. Output: overall verdict (APPROVE/BLOCK), blocking issues, non-blocking notes, recommendations.".into(),
            vec!["review_rct", "review_integration", "review_spec"],
            120_000, // 2 min
        ));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec {
            description: Some("RCT: plan → implement → parallel gate review → aggregate".into()),
            steps,
        });

        let mut profile_map = BTreeMap::new();
        for (name, ..) in &agents {
            profile_map.insert(name.to_string(), ProfileName::from(*name));
        }

        MobDefinition {
            id: MobId::from(format!("force-rct-{}", uuid::Uuid::new_v4().as_simple())),
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
