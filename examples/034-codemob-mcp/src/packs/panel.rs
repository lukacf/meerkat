//! Panel pack — free-form debate with 5 opinionated agents and a moderator.
//!
//! Unlike flow-based packs, the panel uses **comms-based** execution:
//! all agents are `autonomous_host` with full-mesh wiring. The moderator
//! controls the floor, and all agents obey moderator instructions absolutely.
//!
//! The deliberate handler detects `flow_step_count() == 0` and routes to
//! `run_comms()` instead of `run_flow()`. The task is injected via
//! `mob_member_send` to the moderator (not baked into a flow step), which
//! is why `_task` and `_context` are unused in `definition()`.

use std::collections::BTreeMap;

use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;
use serde_json::Value;

use super::{Pack, resolve_model};

pub struct PanelPack;

impl Pack for PanelPack {
    fn name(&self) -> &str { "panel" }
    fn description(&self) -> &str { "Free-form review panel: 5 opinionated agents debate with a moderator keeping order. Full mesh comms, no flow script." }
    fn agent_count(&self) -> usize { 5 }
    fn flow_step_count(&self) -> usize { 0 } // comms-driven, no flow

    fn definition(&self, _task: &str, _context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        // Diverse models across providers for genuine perspective differences
        let agents: Vec<(&str, &str, &str, String)> = vec![
            ("moderator",  "moderator-skill",  "Neutral moderator — keeps discussion productive", resolve_model(overrides, "moderator", "claude-opus-4-6")),
            ("purist",     "purist-skill",     "Architecture purist — demands clean design",      resolve_model(overrides, "purist", "gemini-3.1-pro-preview")),
            ("pragmatist", "pragmatist-skill", "Pragmatist — ship it and iterate",                resolve_model(overrides, "pragmatist", "gpt-5.2")),
            ("skeptic",    "skeptic-skill",    "Skeptic — doubts everything, finds edge cases",   resolve_model(overrides, "skeptic", "gemini-3-flash-preview")),
            ("veteran",    "veteran-skill",    "Industry veteran — 20 years of war stories",      resolve_model(overrides, "veteran", "gpt-5.3-codex")),
        ];

        let tools = ToolConfig { comms: true, ..ToolConfig::default() };

        let mut profiles = BTreeMap::new();
        for (name, skill, desc, m) in &agents {
            profiles.insert(ProfileName::from(*name), Profile {
                model: m.clone(),
                skills: vec![skill.to_string()],
                tools: tools.clone(),
                peer_description: desc.to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: pp.cloned(),
            });
        }

        let mut skills = BTreeMap::new();
        skills.insert("moderator-skill".into(),  SkillSource::Inline { content: include_str!("../../skills/moderator.md").into() });
        skills.insert("purist-skill".into(),     SkillSource::Inline { content: include_str!("../../skills/purist.md").into() });
        skills.insert("pragmatist-skill".into(), SkillSource::Inline { content: include_str!("../../skills/pragmatist.md").into() });
        skills.insert("skeptic-skill".into(),    SkillSource::Inline { content: include_str!("../../skills/skeptic.md").into() });
        skills.insert("veteran-skill".into(),    SkillSource::Inline { content: include_str!("../../skills/veteran.md").into() });

        // Full mesh: every pair wired (agents can message anyone)
        let names: Vec<&str> = agents.iter().map(|(n, ..)| *n).collect();
        let mut role_wiring = Vec::new();
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                role_wiring.push(RoleWiringRule {
                    a: ProfileName::from(names[i]),
                    b: ProfileName::from(names[j]),
                });
            }
        }

        MobDefinition {
            id: MobId::from(format!("codemob-panel-{}", uuid::Uuid::new_v4().as_simple())),
            orchestrator: Some(OrchestratorConfig { profile: ProfileName::from("moderator") }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules { auto_wire_orchestrator: true, role_wiring },
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
