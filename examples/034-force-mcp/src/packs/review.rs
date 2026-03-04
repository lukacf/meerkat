use std::collections::BTreeMap;

use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::{Pack, text_schema};

pub struct ReviewPack;

impl Pack for ReviewPack {
    fn name(&self) -> &str {
        "review"
    }

    fn description(&self) -> &str {
        "Parallel code review: general + security + performance, then synthesis"
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
            format!("\n\n## Code / Context\n\n{context}")
        };

        let model = |role: &str, default: &str| -> String {
            model_overrides
                .get(role)
                .cloned()
                .unwrap_or_else(|| default.to_string())
        };

        let profile = |role: &str, skill: &str, desc: &str, m: &str| -> Profile {
            Profile {
                model: m.to_string(),
                skills: vec![skill.to_string()],
                tools: ToolConfig { comms: true, ..ToolConfig::default() },
                peer_description: desc.to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: Some(text_schema()),
                provider_params: None,
            }
        };

        let mut profiles = BTreeMap::new();
        profiles.insert(ProfileName::from("reviewer"), profile("reviewer", "reviewer-skill", "General code reviewer", &model("reviewer", "claude-sonnet-4-5")));
        profiles.insert(ProfileName::from("security"), profile("security", "security-skill", "Security-focused reviewer", &model("security", "claude-sonnet-4-5")));
        profiles.insert(ProfileName::from("perf"), profile("perf", "perf-skill", "Performance-focused reviewer", &model("perf", "claude-sonnet-4-5")));
        profiles.insert(ProfileName::from("synthesizer"), profile("synthesizer", "synthesizer-skill", "Review synthesizer", &model("synthesizer", "claude-sonnet-4-5")));

        let mut skills = BTreeMap::new();
        skills.insert("reviewer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/reviewer.md").into() });
        skills.insert("security-skill".into(), SkillSource::Inline { content: include_str!("../../skills/security_reviewer.md").into() });
        skills.insert("perf-skill".into(), SkillSource::Inline { content: include_str!("../../skills/perf_reviewer.md").into() });
        skills.insert("synthesizer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/synthesizer.md").into() });

        let review_msg = format!("Review the following:\n\n{task}{ctx}");

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("general_review"), FlowStepSpec {
            role: ProfileName::from("reviewer"),
            message: review_msg.clone(),
            depends_on: vec![],
            dispatch_mode: DispatchMode::default(),
            collection_policy: CollectionPolicy::default(),
            condition: None,
            timeout_ms: Some(120_000),
            expected_schema_ref: None,
            branch: None,
            depends_on_mode: DependencyMode::default(),
            allowed_tools: None,
            blocked_tools: None,
        });
        steps.insert(StepId::from("security_review"), FlowStepSpec {
            role: ProfileName::from("security"),
            message: format!("Focus on security aspects:\n\n{task}{ctx}"),
            depends_on: vec![],
            ..steps.get(&StepId::from("general_review")).unwrap().clone()
        });
        steps.insert(StepId::from("perf_review"), FlowStepSpec {
            role: ProfileName::from("perf"),
            message: format!("Focus on performance aspects:\n\n{task}{ctx}"),
            depends_on: vec![],
            ..steps.get(&StepId::from("general_review")).unwrap().clone()
        });
        steps.insert(StepId::from("synthesize"), FlowStepSpec {
            role: ProfileName::from("synthesizer"),
            message: "Synthesize all review findings below into a unified code review assessment. Include: summary, critical issues, recommendations, overall verdict (approve/request changes/reject).\n\n## General Review\n{{ steps.general_review }}\n\n## Security Review\n{{ steps.security_review }}\n\n## Performance Review\n{{ steps.perf_review }}".into(),
            depends_on: vec![StepId::from("general_review"), StepId::from("security_review"), StepId::from("perf_review")],
            ..steps.get(&StepId::from("general_review")).unwrap().clone()
        });

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec {
            description: Some("Parallel code review with synthesis".into()),
            steps,
        });

        let mut profile_map = BTreeMap::new();
        for name in ["reviewer", "security", "perf", "synthesizer"] {
            profile_map.insert(name.into(), ProfileName::from(name));
        }

        MobDefinition {
            id: MobId::from(format!("force-review-{}", uuid::Uuid::new_v4().as_simple())),
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
