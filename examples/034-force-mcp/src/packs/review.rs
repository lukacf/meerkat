//! Review pack — 3 parallel specialized reviewers + synthesizer.
//!
//! General, security, and performance reviewers run in parallel (no
//! dependencies between them). The synthesizer receives all 3 outputs
//! via `{{ steps.<id> }}` template references and produces a unified assessment.

use std::collections::BTreeMap;
use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use serde_json::Value;

use super::*;

pub struct ReviewPack;

impl Pack for ReviewPack {
    fn name(&self) -> &str { "review" }
    fn description(&self) -> &str { "Parallel code review: general + security + performance, then synthesis" }
    fn agent_count(&self) -> usize { 4 }
    fn flow_step_count(&self) -> usize { 4 }

    fn definition(&self, task: &str, context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let ctx = format_context(context);

        let agents = [
            ("reviewer",    "reviewer-skill",          "General code reviewer",          "claude-sonnet-4-6"),
            ("security",    "security-skill",          "Security-focused reviewer",      "claude-sonnet-4-6"),
            ("perf",        "perf-skill",              "Performance-focused reviewer",   "claude-sonnet-4-6"),
            ("synthesizer", "synthesizer-skill",       "Review synthesizer",             "claude-sonnet-4-6"),
        ];

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, default) in &agents {
            profiles.insert(ProfileName::from(*name), turn_driven_profile(resolve_model(overrides, name, default), skill, desc, pp));
        }

        let mut skills = BTreeMap::new();
        skills.insert("reviewer-skill".into(),    SkillSource::Inline { content: include_str!("../../skills/reviewer.md").into() });
        skills.insert("security-skill".into(),    SkillSource::Inline { content: include_str!("../../skills/security_reviewer.md").into() });
        skills.insert("perf-skill".into(),        SkillSource::Inline { content: include_str!("../../skills/perf_reviewer.md").into() });
        skills.insert("synthesizer-skill".into(), SkillSource::Inline { content: include_str!("../../skills/synthesizer.md").into() });

        let review_msg = format!("Review the following:\n\n{task}{ctx}");

        let mut steps = IndexMap::new();
        steps.insert(StepId::from("general_review"),  flow_step("reviewer",  review_msg.clone(), &[], 120_000));
        steps.insert(StepId::from("security_review"), flow_step("security",  format!("Focus on security aspects:\n\n{task}{ctx}"), &[], 120_000));
        steps.insert(StepId::from("perf_review"),     flow_step("perf",      format!("Focus on performance aspects:\n\n{task}{ctx}"), &[], 120_000));
        steps.insert(StepId::from("synthesize"), flow_step("synthesizer",
            "Synthesize all review findings below into a unified assessment. Include: summary, critical issues, recommendations, verdict (approve/request changes/reject).\n\n\
             ## General Review\n{{ steps.general_review }}\n\n## Security Review\n{{ steps.security_review }}\n\n## Performance Review\n{{ steps.perf_review }}".into(),
            &["general_review", "security_review", "perf_review"], 120_000));

        let mut flows = BTreeMap::new();
        flows.insert(FlowId::from("main"), FlowSpec { description: Some("Parallel code review with synthesis".into()), steps });

        let names: Vec<&str> = agents.iter().map(|(n, ..)| *n).collect();
        mob_definition("review", profiles, skills, flows, identity_spawn_policy(&names))
    }
}
