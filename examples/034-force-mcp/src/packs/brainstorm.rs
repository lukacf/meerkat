//! Brainstorm pack — 3 diverse-model ideators + synthesizer.
//!
//! Ideators run in parallel with intentionally different default models
//! (Anthropic, OpenAI, Gemini) for perspective diversity. The synthesizer
//! receives all 3 outputs and produces a ranked list of ideas.

use std::collections::BTreeMap;
use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use serde_json::Value;

use super::*;

pub struct BrainstormPack;

impl Pack for BrainstormPack {
    fn name(&self) -> &str { "brainstorm" }
    fn description(&self) -> &str { "Multi-model ideation: 3 diverse perspectives + synthesis with ranked ideas" }
    fn agent_count(&self) -> usize { 4 }
    fn flow_step_count(&self) -> usize { 4 }

    fn definition(&self, task: &str, context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let ctx = format_context(context);

        // Intentionally diverse default models for different perspectives
        let agents = [
            ("ideator_a",   "ideator-a-skill",   "Practical ideator",   "gemini-3.1-pro-preview"),
            ("ideator_b",   "ideator-b-skill",   "Creative ideator",    "gpt-5.2"),
            ("ideator_c",   "ideator-c-skill",   "Contrarian ideator",  "gemini-3-flash-preview"),
            ("synthesizer", "synthesizer-skill",  "Idea synthesizer",    "claude-opus-4-6"),
        ];

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, default) in &agents {
            profiles.insert(ProfileName::from(*name), turn_driven_profile(resolve_model(overrides, name, default), skill, desc, pp));
        }

        let mut skills = BTreeMap::new();
        skills.insert("ideator-a-skill".into(),   SkillSource::Inline { content: include_str!("../../skills/ideator_a.md").into() });
        skills.insert("ideator-b-skill".into(),   SkillSource::Inline { content: include_str!("../../skills/ideator_b.md").into() });
        skills.insert("ideator-c-skill".into(),   SkillSource::Inline { content: include_str!("../../skills/ideator_c.md").into() });
        skills.insert("synthesizer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/synthesizer.md").into() });

        let ideate_msg = format!("Generate ideas for:\n\n{task}{ctx}");

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("ideate_a"), flow_step("ideator_a", ideate_msg.clone(), &[], 120_000));
        steps.insert(StepId::from("ideate_b"), flow_step("ideator_b", ideate_msg.clone(), &[], 120_000));
        steps.insert(StepId::from("ideate_c"), flow_step("ideator_c", ideate_msg, &[], 120_000));
        steps.insert(StepId::from("synthesize"), flow_step("synthesizer",
            "Synthesize all ideas below into a ranked list. For each: describe, assess feasibility, note trade-offs. Recommend top 2-3.\n\n\
             ## Practical\n{{ steps.ideate_a }}\n\n## Creative\n{{ steps.ideate_b }}\n\n## Contrarian\n{{ steps.ideate_c }}".into(),
            &["ideate_a", "ideate_b", "ideate_c"], 120_000));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Multi-perspective brainstorm".into()), steps });

        let names: Vec<&str> = agents.iter().map(|(n, ..)| *n).collect();
        mob_definition("brainstorm", profiles, skills, flows, identity_spawn_policy(&names))
    }
}
