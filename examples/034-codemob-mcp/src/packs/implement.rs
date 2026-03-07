//! Implement pack — gated implementation with iterative review.
//!
//! Two agents in a comms-based review loop:
//! - **implementer**: receives the task, produces the solution
//! - **reviewer**: quality gate — approves or sends feedback for revision
//!
//! The reviewer acts as orchestrator. It receives the task, forwards it to
//! the implementer, reviews the output, and either approves (captured as
//! the final result) or sends feedback for another iteration. Capped at
//! 3 revision rounds by the reviewer's skill instructions.

use std::collections::BTreeMap;

use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;
use serde_json::Value;

use super::{Pack, resolve_model};

pub struct ImplementPack;

impl Pack for ImplementPack {
    fn name(&self) -> &str { "implement" }
    fn description(&self) -> &str { "Gated implementation: an implementer produces work, a reviewer approves or sends feedback. Iterates until the reviewer is satisfied (max 3 rounds)." }
    fn agent_count(&self) -> usize { 2 }
    fn flow_step_count(&self) -> usize { 0 } // comms-driven review loop

    fn definition(&self, _task: &str, _context: &str, overrides: &BTreeMap<String, String>, pp: Option<&Value>) -> MobDefinition {
        let tools = ToolConfig { comms: true, ..ToolConfig::default() };

        let mut profiles = BTreeMap::new();
        profiles.insert(ProfileName::from("implementer"), Profile {
            model: resolve_model(overrides, "implementer", "claude-sonnet-4-6"),
            skills: vec!["implementer-skill".to_string()],
            tools: tools.clone(),
            peer_description: "Implementation agent — produces the solution".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: pp.cloned(),
        });
        profiles.insert(ProfileName::from("reviewer"), Profile {
            model: resolve_model(overrides, "reviewer", "gpt-5.3-codex"),
            skills: vec!["gate-reviewer-skill".to_string()],
            tools: tools.clone(),
            peer_description: "Quality gate reviewer — approves or requests revision".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: pp.cloned(),
        });

        let mut skills = BTreeMap::new();
        skills.insert("implementer-skill".into(), SkillSource::Inline {
            content: include_str!("../../skills/implementer.md").into(),
        });
        skills.insert("gate-reviewer-skill".into(), SkillSource::Inline {
            content: include_str!("../../skills/gate_reviewer.md").into(),
        });

        MobDefinition {
            id: MobId::from(format!("codemob-implement-{}", uuid::Uuid::new_v4().as_simple())),
            orchestrator: Some(OrchestratorConfig { profile: ProfileName::from("reviewer") }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules {
                auto_wire_orchestrator: true,
                role_wiring: vec![RoleWiringRule {
                    a: ProfileName::from("implementer"),
                    b: ProfileName::from("reviewer"),
                }],
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
