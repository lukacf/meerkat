//! Advisor pack — single agent, quick opinion (simplest possible pack).
//!
//! Exists as a minimal "hello world" for the pack pattern. For single-agent
//! use without mob overhead, the `consult` tool is more efficient.

use std::collections::BTreeMap;
use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use serde_json::Value;

use super::*;

pub struct AdvisorPack;

impl Pack for AdvisorPack {
    fn name(&self) -> &str { "advisor" }
    fn description(&self) -> &str { "Single agent, quick opinion — like asking a colleague" }
    fn agent_count(&self) -> usize { 1 }
    fn flow_step_count(&self) -> usize { 1 }

    fn definition(&self, task: &str, context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let ctx = format_context(context);

        let mut profiles = BTreeMap::new();
        profiles.insert(ProfileName::from("advisor"),
            turn_driven_profile(resolve_model(overrides, "advisor", "gpt-5.3-codex"), "advisor-skill", "Technical advisor", pp));

        let mut skills = BTreeMap::new();
        skills.insert("advisor-skill".into(), SkillSource::Inline { content: include_str!("../../skills/advisor.md").into() });

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("respond"), flow_step("advisor", format!("{task}{ctx}"), &[], 120_000));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Single-agent advisory opinion".into()), steps });

        mob_definition("advisor", profiles, skills, flows, identity_spawn_policy(&["advisor"]))
    }
}
