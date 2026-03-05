//! Red-team pack — adversarial risk assessment.
//!
//! Advocate and adversary argue in parallel (no dependency between them),
//! then the judge receives both arguments via template references and
//! produces a balanced verdict.

use std::collections::BTreeMap;
use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use serde_json::Value;

use super::*;

pub struct RedTeamPack;

impl Pack for RedTeamPack {
    fn name(&self) -> &str { "red-team" }
    fn description(&self) -> &str { "Balanced risk assessment: advocate argues for, adversary argues against, judge decides" }
    fn agent_count(&self) -> usize { 3 }
    fn flow_step_count(&self) -> usize { 3 }

    fn definition(&self, task: &str, context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let ctx = format_context(context);

        let mut profiles = BTreeMap::new();
        // Different providers for advocate/adversary ensures genuinely different reasoning
        profiles.insert(ProfileName::from("advocate"),  turn_driven_profile(resolve_model(overrides, "advocate", "gemini-3-flash-preview"), "advocate-skill", "Advocate — argues in favor", pp));
        profiles.insert(ProfileName::from("adversary"), turn_driven_profile(resolve_model(overrides, "adversary", "gpt-5.2"), "adversary-skill", "Adversary — argues against", pp));
        profiles.insert(ProfileName::from("judge"),     turn_driven_profile(resolve_model(overrides, "judge", "claude-opus-4-6"), "judge-skill", "Judge — balanced assessment", pp));

        let mut skills = BTreeMap::new();
        skills.insert("advocate-skill".into(),  SkillSource::Inline { content: include_str!("../../skills/advocate.md").into() });
        skills.insert("adversary-skill".into(), SkillSource::Inline { content: include_str!("../../skills/adversary.md").into() });
        skills.insert("judge-skill".into(),     SkillSource::Inline { content: include_str!("../../skills/judge.md").into() });

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("advocate_case"),  flow_step("advocate",  format!("Argue IN FAVOR:\n\n{task}{ctx}"), &[], 120_000));
        steps.insert(StepId::from("adversary_case"), flow_step("adversary", format!("Argue AGAINST:\n\n{task}{ctx}"), &[], 120_000));
        steps.insert(StepId::from("judgment"), flow_step("judge",
            "Weigh both arguments and produce a balanced risk assessment with a clear recommendation.\n\n\
             ## Advocate (Pro)\n{{ steps.advocate_case }}\n\n## Adversary (Con)\n{{ steps.adversary_case }}".into(),
            &["advocate_case", "adversary_case"], 120_000));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Adversarial risk assessment".into()), steps });

        mob_definition("redteam", profiles, skills, flows, identity_spawn_policy(&["advocate", "adversary", "judge"]))
    }
}
