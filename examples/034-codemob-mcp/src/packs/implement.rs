//! Implement pack — gated implementation with reviewer synthesis.
//!
//! Two agents run in a `"main"` flow: the implementer produces the solution,
//! then the reviewer emits the terminal gate verdict. Completion is resolved by
//! `MobMachine` flow terminality.

use indexmap::IndexMap;
use std::collections::BTreeMap;

use meerkat_mob::definition::*;
use meerkat_mob::ids::*;

use super::{flow_step, format_context, identity_spawn_policy, mob_definition, resolve_model, turn_driven_profile, Pack};

pub struct ImplementPack;

impl Pack for ImplementPack {
    fn name(&self) -> &str {
        "implement"
    }
    fn description(&self) -> &str {
        "Gated implementation: an implementer produces work, then a reviewer emits a gate verdict"
    }
    fn agent_count(&self) -> usize {
        2
    }
    fn flow_step_count(&self) -> usize {
        2
    }

    fn definition(
        &self,
        task: &str,
        context: &str,
        overrides: &BTreeMap<String, String>,
        pp: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> MobDefinition {
        let ctx = format_context(context);

        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("implementer"),
            turn_driven_profile(
                resolve_model(overrides, "implementer", "claude-sonnet-4-6"),
                "implementer-skill",
                "Implementation agent — produces the solution",
                pp,
            ),
        );
        profiles.insert(
            ProfileName::from("reviewer"),
            turn_driven_profile(
                resolve_model(overrides, "reviewer", "gpt-5.5"),
                "gate-reviewer-skill",
                "Quality gate reviewer — approves or requests revision",
                pp,
            ),
        );

        let mut skills = BTreeMap::new();
        skills.insert(
            "implementer-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/implementer.md").into(),
            },
        );
        skills.insert(
            "gate-reviewer-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/gate_reviewer.md").into(),
            },
        );

        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("implement"),
            flow_step(
                "implementer",
                format!("Implement the requested change. Include the approach, changed files, and verification performed.\n\n{task}{ctx}"),
                &[],
                600_000,
            ),
        );
        steps.insert(
            StepId::from("review"),
            flow_step(
                "reviewer",
                "Review the implementation below. Produce a final gate verdict: APPROVE or BLOCK, with blocking issues first and any follow-up recommendations.\n\n## Implementation\n{{ steps.implement }}".into(),
                &["implement"],
                300_000,
            ),
        );

        let mut flows = BTreeMap::new();
        flows.insert(
            FlowId::from("main"),
            FlowSpec::new(Some("Implementation followed by gate review".into()), steps, None),
        );

        mob_definition(
            "implement",
            profiles,
            skills,
            flows,
            identity_spawn_policy(&["implementer", "reviewer"]),
        )
    }
}
