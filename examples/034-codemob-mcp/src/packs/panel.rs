//! Panel pack — structured debate with opinionated agents and a moderator.
//!
//! The panel is a `"main"` flow: the moderator frames the task, the panelists
//! respond in parallel, and the moderator synthesizes. Completion is therefore
//! owned by `MobMachine`, not by event-stream quiescence heuristics.

use indexmap::IndexMap;
use std::collections::BTreeMap;

use meerkat_mob::definition::*;
use meerkat_mob::ids::*;

use super::{flow_step, format_context, identity_spawn_policy, mob_definition, resolve_model, turn_driven_profile, Pack};

pub struct PanelPack;

impl Pack for PanelPack {
    fn name(&self) -> &str {
        "panel"
    }
    fn description(&self) -> &str {
        "Structured review panel: opinionated agents respond in parallel, then a moderator synthesizes"
    }
    fn agent_count(&self) -> usize {
        5
    }
    fn flow_step_count(&self) -> usize {
        6
    }

    fn definition(
        &self,
        task: &str,
        context: &str,
        overrides: &BTreeMap<String, String>,
        pp: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> MobDefinition {
        let ctx = format_context(context);

        // Diverse models across providers for genuine perspective differences
        let agents: Vec<(&str, &str, &str, String)> = vec![
            (
                "moderator",
                "moderator-skill",
                "Neutral moderator — keeps discussion productive",
                resolve_model(overrides, "moderator", "claude-opus-4-8"),
            ),
            (
                "purist",
                "purist-skill",
                "Architecture purist — demands clean design",
                resolve_model(overrides, "purist", "gemini-3.1-pro-preview"),
            ),
            (
                "pragmatist",
                "pragmatist-skill",
                "Pragmatist — ship it and iterate",
                resolve_model(overrides, "pragmatist", "gpt-5.5"),
            ),
            (
                "skeptic",
                "skeptic-skill",
                "Skeptic — doubts everything, finds edge cases",
                resolve_model(overrides, "skeptic", "gemini-3.1-flash-lite-preview"),
            ),
            (
                "veteran",
                "veteran-skill",
                "Industry veteran — 20 years of war stories",
                resolve_model(overrides, "veteran", "claude-sonnet-4-6"),
            ),
        ];

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, m) in &agents {
            profiles.insert(
                ProfileName::from(*name),
                turn_driven_profile(m.clone(), skill, desc, pp),
            );
        }

        let mut skills = BTreeMap::new();
        skills.insert(
            "moderator-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/moderator.md").into(),
            },
        );
        skills.insert(
            "purist-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/purist.md").into(),
            },
        );
        skills.insert(
            "pragmatist-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/pragmatist.md").into(),
            },
        );
        skills.insert(
            "skeptic-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/skeptic.md").into(),
            },
        );
        skills.insert(
            "veteran-skill".into(),
            SkillSource::Inline {
                content: include_str!("../../skills/veteran.md").into(),
            },
        );

        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("moderator_brief"),
            flow_step(
                "moderator",
                format!("Frame this panel discussion. Identify the key question, constraints, and what each panelist should pressure-test.\n\n{task}{ctx}"),
                &[],
                300_000,
            ),
        );
        steps.insert(
            StepId::from("purist_case"),
            flow_step(
                "purist",
                "Respond as the architecture purist. Name clean-design risks, boundary violations, and the purest path forward.\n\n## Moderator Brief\n{{ steps.moderator_brief }}".into(),
                &["moderator_brief"],
                300_000,
            ),
        );
        steps.insert(
            StepId::from("pragmatist_case"),
            flow_step(
                "pragmatist",
                "Respond as the pragmatist. Name the shortest shippable path, acceptable compromises, and what not to overbuild.\n\n## Moderator Brief\n{{ steps.moderator_brief }}".into(),
                &["moderator_brief"],
                300_000,
            ),
        );
        steps.insert(
            StepId::from("skeptic_case"),
            flow_step(
                "skeptic",
                "Respond as the skeptic. Find edge cases, weak assumptions, failure modes, and missing evidence.\n\n## Moderator Brief\n{{ steps.moderator_brief }}".into(),
                &["moderator_brief"],
                300_000,
            ),
        );
        steps.insert(
            StepId::from("veteran_case"),
            flow_step(
                "veteran",
                "Respond as the veteran. Bring operational lessons, maintainability concerns, and long-term cost trade-offs.\n\n## Moderator Brief\n{{ steps.moderator_brief }}".into(),
                &["moderator_brief"],
                300_000,
            ),
        );
        steps.insert(
            StepId::from("moderator_synthesis"),
            flow_step(
                "moderator",
                "Synthesize the panel into a clear verdict. Include: core tension, strongest agreements, unresolved risks, and recommended next action.\n\n\
                 ## Purist\n{{ steps.purist_case }}\n\n## Pragmatist\n{{ steps.pragmatist_case }}\n\n## Skeptic\n{{ steps.skeptic_case }}\n\n## Veteran\n{{ steps.veteran_case }}".into(),
                &["purist_case", "pragmatist_case", "skeptic_case", "veteran_case"],
                300_000,
            ),
        );

        let mut flows = BTreeMap::new();
        flows.insert(
            FlowId::from("main"),
            FlowSpec::new(Some("Structured panel debate with moderator synthesis".into()), steps, None),
        );

        let names: Vec<&str> = agents.iter().map(|(n, ..)| *n).collect();
        mob_definition(
            "panel",
            profiles,
            skills,
            flows,
            identity_spawn_policy(&names),
        )
    }
}
