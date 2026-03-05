//! RCT pack — full Representation Contract Test pipeline.
//!
//! 6 agents: orchestrator plans, implementer codes, 3 reviewers gate in
//! parallel (RCT Guardian, Integration Sheriff, Spec Auditor), aggregator
//! produces the final verdict. Reviewers have shell access to run tests
//! independently (no status echoing).

use std::collections::BTreeMap;
use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;
use serde_json::Value;

use super::*;

pub struct RctPack;

impl Pack for RctPack {
    fn name(&self) -> &str { "rct" }
    fn description(&self) -> &str { "Full RCT pipeline: plan, implement, 3 parallel gate reviewers, aggregate verdict" }
    fn agent_count(&self) -> usize { 6 }
    fn flow_step_count(&self) -> usize { 6 }

    fn definition(&self, task: &str, context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let ctx = format_context(context);

        let tools_with_shell = ToolConfig { builtins: true, shell: true, comms: true, ..ToolConfig::default() };
        let tools_comms_only = ToolConfig { comms: true, ..ToolConfig::default() };

        // (name, skill, description, default_model, tools, mode)
        let agents: Vec<(&str, &str, &str, &str, ToolConfig)> = vec![
            // 6 agents, 6 distinct models — every agent sees the code differently
            ("orchestrator",       "rct-orchestrator-skill", "RCT pipeline orchestrator",      "claude-opus-4-6",        tools_with_shell.clone()),
            ("implementer",        "rct-implementer-skill",  "Implementation agent",            "gpt-5.3-codex",         tools_with_shell.clone()),
            ("rct_guardian",       "rct-guardian-skill",     "RCT Guardian reviewer",           "gemini-3.1-pro-preview", tools_with_shell.clone()),
            ("integration_sheriff","rct-sheriff-skill",      "Integration Sheriff reviewer",    "gpt-5.2",               tools_with_shell.clone()),
            ("spec_auditor",       "rct-auditor-skill",      "Spec Auditor reviewer",           "gemini-3-flash-preview", tools_with_shell.clone()),
            ("aggregator",         "rct-aggregator-skill",   "Gate verdict aggregator",         "claude-sonnet-4-6",      tools_comms_only),
        ];

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, default, tools) in &agents {
            profiles.insert(ProfileName::from(*name), Profile {
                model: resolve_model(overrides, name, default),
                skills: vec![skill.to_string()],
                tools: tools.clone(),
                peer_description: desc.to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: pp.cloned(),
            });
        }

        let mut skills = BTreeMap::new();
        skills.insert("rct-orchestrator-skill".into(), SkillSource::Inline { content: include_str!("../../skills/rct_orchestrator.md").into() });
        skills.insert("rct-implementer-skill".into(),  SkillSource::Inline { content: include_str!("../../skills/rct_implementer.md").into() });
        skills.insert("rct-guardian-skill".into(),     SkillSource::Inline { content: include_str!("../../skills/rct_guardian.md").into() });
        skills.insert("rct-sheriff-skill".into(),      SkillSource::Inline { content: include_str!("../../skills/rct_integration_sheriff.md").into() });
        skills.insert("rct-auditor-skill".into(),      SkillSource::Inline { content: include_str!("../../skills/rct_spec_auditor.md").into() });
        skills.insert("rct-aggregator-skill".into(),   SkillSource::Inline { content: include_str!("../../skills/rct_aggregator.md").into() });

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("plan"), flow_step("orchestrator",
            format!("Analyze this task and create an implementation plan. Create .rct/spec.yaml and .rct/checklist.yaml.\n\n{task}{ctx}"),
            &[], 300_000));
        steps.insert(StepId::from("implement"), flow_step("implementer",
            "Implement the plan. Read .rct/checklist.yaml for tasks. Run verification commands. Mark tasks done.\n\n## Plan\n{{ steps.plan }}".into(),
            &["plan"], 600_000));
        // 3 reviewers run in parallel (all depend on implement, no deps between them)
        steps.insert(StepId::from("review_rct"),         flow_step("rct_guardian",       "Gate review: Check representation contracts. Run tests independently. Produce verdict.".into(), &["implement"], 180_000));
        steps.insert(StepId::from("review_integration"), flow_step("integration_sheriff","Gate review: Check cross-component wiring. Run tests independently. Produce verdict.".into(), &["implement"], 180_000));
        steps.insert(StepId::from("review_spec"),        flow_step("spec_auditor",       "Gate review: Check requirements compliance against .rct/spec.yaml. Produce verdict.".into(), &["implement"], 180_000));
        steps.insert(StepId::from("aggregate"), flow_step("aggregator",
            "Aggregate the 3 reviewer verdicts into a final gate result: APPROVE/BLOCK, blocking issues, recommendations.\n\n\
             ## RCT Guardian\n{{ steps.review_rct }}\n\n## Integration Sheriff\n{{ steps.review_integration }}\n\n## Spec Auditor\n{{ steps.review_spec }}".into(),
            &["review_rct", "review_integration", "review_spec"], 120_000));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec {
            description: Some("RCT: plan → implement → parallel gate review → aggregate".into()), steps });

        let names: Vec<&str> = agents.iter().map(|(n, ..)| *n).collect();
        mob_definition("rct", profiles, skills, flows, identity_spawn_policy(&names))
    }
}
