//! Architect pack — sequential deliberation with plan, critique, revision, and ADR.
//!
//! The planner proposes, the critic challenges, the planner revises, and the
//! synthesizer produces a final Architecture Decision Record. Each step depends
//! on the previous one — no parallelism.

use std::collections::BTreeMap;
use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use serde_json::Value;

use super::*;

pub struct ArchitectPack;

impl Pack for ArchitectPack {
    fn name(&self) -> &str { "architect" }
    fn description(&self) -> &str { "Design deliberation: planner proposes, critic challenges, synthesizer produces ADR" }
    fn agent_count(&self) -> usize { 3 }
    fn flow_step_count(&self) -> usize { 4 }

    fn definition(&self, task: &str, context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let ctx = format_context(context);

        let mut profiles = BTreeMap::new();
        profiles.insert(ProfileName::from("planner"),     turn_driven_profile(resolve_model(overrides, "planner", "claude-opus-4-6"), "planner-skill", "Architecture planner", pp));
        profiles.insert(ProfileName::from("critic"),      turn_driven_profile(resolve_model(overrides, "critic", "gpt-5.3-codex"), "critic-skill", "Architecture critic", pp));
        profiles.insert(ProfileName::from("synthesizer"), turn_driven_profile(resolve_model(overrides, "synthesizer", "gemini-3.1-pro-preview"), "synthesizer-skill", "Decision record writer", pp));

        let mut skills = BTreeMap::new();
        skills.insert("planner-skill".into(),     SkillSource::Inline { content: include_str!("../../skills/planner.md").into() });
        skills.insert("critic-skill".into(),      SkillSource::Inline { content: include_str!("../../skills/critic.md").into() });
        skills.insert("synthesizer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/synthesizer.md").into() });

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("plan"),       flow_step("planner",     format!("Create an architecture plan for:\n\n{task}{ctx}"), &[], 180_000));
        steps.insert(StepId::from("critique"),   flow_step("critic",      "Critique this plan. Find weaknesses and risky assumptions.\n\n## Plan\n{{ steps.plan }}".into(), &["plan"], 180_000));
        steps.insert(StepId::from("revise"),     flow_step("planner",     "Address the critique. Revise your plan.\n\n## Critique\n{{ steps.critique }}".into(), &["critique"], 180_000));
        steps.insert(StepId::from("synthesize"), flow_step("synthesizer", "Produce a final ADR: context, decision, consequences, alternatives.\n\n## Revised Plan\n{{ steps.revise }}".into(), &["revise"], 180_000));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Architecture deliberation".into()), steps });

        mob_definition("architect", profiles, skills, flows, identity_spawn_policy(&["planner", "critic", "synthesizer"]))
    }
}
