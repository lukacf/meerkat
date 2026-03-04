use std::collections::BTreeMap;

use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::Pack;

pub struct PanelPack;

impl Pack for PanelPack {
    fn name(&self) -> &str {
        "panel"
    }

    fn description(&self) -> &str {
        "Free-form review panel: 5 opinionated agents debate with a moderator keeping order. Full mesh comms, no flow script."
    }

    fn agent_count(&self) -> usize {
        5
    }

    fn flow_step_count(&self) -> usize {
        0 // no flow — comms-driven
    }

    fn definition(
        &self,
        _task: &str,
        _context: &str,
        model_overrides: &BTreeMap<String, String>,
    ) -> MobDefinition {
        let model = |role: &str, default: &str| -> String {
            model_overrides
                .get(role)
                .cloned()
                .unwrap_or_else(|| default.to_string())
        };

        // Diverse models across providers
        let agents: Vec<(&str, &str, &str, String)> = vec![
            ("moderator", "moderator-skill", "Neutral moderator — keeps discussion productive",
             model("moderator", "claude-opus-4-6")),
            ("purist", "purist-skill", "Architecture purist — demands clean design",
             model("purist", "claude-sonnet-4-5")),
            ("pragmatist", "pragmatist-skill", "Pragmatist — ship it and iterate",
             model("pragmatist", "gpt-5.2")),
            ("skeptic", "skeptic-skill", "Skeptic — doubts everything, finds edge cases",
             model("skeptic", "gemini-3-flash-preview")),
            ("veteran", "veteran-skill", "Industry veteran — 20 years of war stories",
             model("veteran", "claude-sonnet-4-5")),
        ];

        let tools = ToolConfig {
            comms: true,
            ..ToolConfig::default()
        };

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, m) in &agents {
            profiles.insert(
                ProfileName::from(*name),
                Profile {
                    model: m.clone(),
                    skills: vec![skill.to_string()],
                    tools: tools.clone(),
                    peer_description: desc.to_string(),
                    external_addressable: true,
                    backend: None,
                    runtime_mode: MobRuntimeMode::AutonomousHost,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                },
            );
        }

        let mut skills = BTreeMap::new();
        skills.insert("moderator-skill".into(), SkillSource::Inline { content: include_str!("../../skills/moderator.md").into() });
        skills.insert("purist-skill".into(), SkillSource::Inline { content: include_str!("../../skills/purist.md").into() });
        skills.insert("pragmatist-skill".into(), SkillSource::Inline { content: include_str!("../../skills/pragmatist.md").into() });
        skills.insert("skeptic-skill".into(), SkillSource::Inline { content: include_str!("../../skills/skeptic.md").into() });
        skills.insert("veteran-skill".into(), SkillSource::Inline { content: include_str!("../../skills/veteran.md").into() });

        // Full mesh: every pair wired
        let agent_names: Vec<&str> = agents.iter().map(|(n, ..)| *n).collect();
        let mut role_wiring = Vec::new();
        for i in 0..agent_names.len() {
            for j in (i + 1)..agent_names.len() {
                role_wiring.push(RoleWiringRule {
                    a: ProfileName::from(agent_names[i]),
                    b: ProfileName::from(agent_names[j]),
                });
            }
        }

        MobDefinition {
            id: MobId::from(format!("force-panel-{}", uuid::Uuid::new_v4().as_simple())),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("moderator"),
            }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules {
                auto_wire_orchestrator: true,
                role_wiring,
            },
            skills,
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
        }
    }
}
