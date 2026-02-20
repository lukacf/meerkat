//! Built-in prefab mob definitions.

use crate::definition::{
    BackendConfig, MobDefinition, OrchestratorConfig, SkillSource, WiringRules,
};
use crate::ids::{MobId, ProfileName};
use crate::profile::{Profile, ToolConfig};
use std::collections::BTreeMap;

/// Built-in prefab templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefab {
    CodingSwarm,
    CodeReview,
    ResearchTeam,
    Pipeline,
}

impl Prefab {
    /// Stable prefab key used in CLI and MCP inputs.
    pub fn key(self) -> &'static str {
        match self {
            Self::CodingSwarm => "coding_swarm",
            Self::CodeReview => "code_review",
            Self::ResearchTeam => "research_team",
            Self::Pipeline => "pipeline",
        }
    }

    /// Parse prefab key.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "coding_swarm" => Some(Self::CodingSwarm),
            "code_review" => Some(Self::CodeReview),
            "research_team" => Some(Self::ResearchTeam),
            "pipeline" => Some(Self::Pipeline),
            _ => None,
        }
    }

    /// All known prefabs.
    pub fn all() -> [Self; 4] {
        [
            Self::CodingSwarm,
            Self::CodeReview,
            Self::ResearchTeam,
            Self::Pipeline,
        ]
    }

    /// Build a full mob definition for this prefab.
    pub fn definition(self) -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            Profile {
                model: "claude-opus-4-6".to_string(),
                skills: vec!["orchestrator".to_string()],
                tools: ToolConfig {
                    builtins: true,
                    comms: true,
                    mob: true,
                    mob_tasks: true,
                    ..ToolConfig::default()
                },
                peer_description: "Orchestrator".to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            },
        );
        profiles.insert(
            ProfileName::from("worker"),
            Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: vec!["worker".to_string()],
                tools: ToolConfig {
                    builtins: true,
                    shell: true,
                    comms: true,
                    mob_tasks: true,
                    ..ToolConfig::default()
                },
                peer_description: "Worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            },
        );

        let mut skills = BTreeMap::new();
        skills.insert(
            "orchestrator".to_string(),
            SkillSource::Inline {
                content: orchestrator_skill(self).to_string(),
            },
        );
        skills.insert(
            "worker".to_string(),
            SkillSource::Inline {
                content: worker_skill(self).to_string(),
            },
        );

        MobDefinition {
            id: MobId::from(self.key()),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules {
                auto_wire_orchestrator: true,
                role_wiring: Vec::new(),
            },
            skills,
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
        }
    }

    /// User-editable TOML template for this prefab.
    pub fn toml_template(self) -> &'static str {
        match self {
            Self::CodingSwarm => include_str!("../prefabs/coding_swarm.toml"),
            Self::CodeReview => include_str!("../prefabs/code_review.toml"),
            Self::ResearchTeam => include_str!("../prefabs/research_team.toml"),
            Self::Pipeline => include_str!("../prefabs/pipeline.toml"),
        }
    }
}

fn orchestrator_skill(prefab: Prefab) -> &'static str {
    match prefab {
        Prefab::CodingSwarm => {
            "## Role\nLead a coding swarm.\n\n## Available Tools\nUse mob tools to spawn/retire/wire workers and task tools for shared work.\n\n## Communication\nUse peers() for discovery and PeerRequest for directed work. Handle mob.peer_added and mob.peer_retired events.\n\n## Coordination Pattern\nFan out coding tasks to workers and aggregate results by task ID.\n\n## Worker Lifecycle\nPrefer reusing existing workers; spawn only for uncovered skill gaps; retire idle workers after merge.\n\n## Completion\nWhen all tasks are completed and verified, summarize and call mob.complete()."
        }
        Prefab::CodeReview => {
            "## Role\nCoordinate multi-reviewer code audits.\n\n## Available Tools\nUse mob tools for topology, task tools for review tickets.\n\n## Communication\nRoute code-review requests by specialization; process peer_added/peer_retired updates.\n\n## Coordination Pattern\nParallel review lanes (correctness, integration, quality) with final synthesis.\n\n## Worker Lifecycle\nSpawn reviewers for missing domains, reuse active reviewers, retire on finish.\n\n## Completion\nComplete when all blocking findings are resolved and call mob.complete()."
        }
        Prefab::ResearchTeam => {
            "## Role\nRun structured research with synthesis.\n\n## Available Tools\nUse mob tools to shape the team and task tools to track hypotheses and sources.\n\n## Communication\nAssign research questions via PeerRequest and consolidate updates from peers.\n\n## Coordination Pattern\nDiverge on exploration, converge on synthesis and recommendations.\n\n## Worker Lifecycle\nSpawn domain researchers when required, retire workers with completed lanes.\n\n## Completion\nComplete when all research tasks are closed and a final synthesis is delivered."
        }
        Prefab::Pipeline => {
            "## Role\nDrive staged pipeline execution.\n\n## Available Tools\nUse mob tools for stage workers and task tools for stage handoffs.\n\n## Communication\nUse directed peer requests to pass artifacts from stage to stage.\n\n## Coordination Pattern\nSequential stage progression with explicit dependencies.\n\n## Worker Lifecycle\nSpawn per-stage workers, reuse workers per stage, retire at stage completion.\n\n## Completion\nComplete when terminal stage succeeds and cleanup is done."
        }
    }
}

fn worker_skill(prefab: Prefab) -> &'static str {
    match prefab {
        Prefab::CodingSwarm => "Implement assigned code tasks and report concise diffs.",
        Prefab::CodeReview => "Review assigned changes and report concrete findings with evidence.",
        Prefab::ResearchTeam => {
            "Gather evidence and return sourced summaries for assigned questions."
        }
        Prefab::Pipeline => "Execute your stage deterministically and emit handoff artifacts.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefabs_are_parseable_and_stable() {
        for prefab in Prefab::all() {
            assert_eq!(Prefab::from_key(prefab.key()), Some(prefab));
        }
        assert_eq!(Prefab::from_key("unknown"), None);
    }

    #[test]
    fn test_prefab_definitions_validate() {
        for prefab in Prefab::all() {
            let def = prefab.definition();
            let diagnostics = crate::validate::validate_definition(&def);
            assert!(
                diagnostics.is_empty(),
                "prefab {} must validate: {diagnostics:?}",
                prefab.key()
            );
        }
    }

    #[test]
    fn test_prefab_orchestrator_skill_contract_sections() {
        let required = [
            "## Role",
            "## Available Tools",
            "## Communication",
            "## Coordination Pattern",
            "## Worker Lifecycle",
            "## Completion",
        ];
        for prefab in Prefab::all() {
            let def = prefab.definition();
            let orchestrator = match &def.skills["orchestrator"] {
                SkillSource::Inline { content } => content,
                SkillSource::Path { .. } => panic!("orchestrator skill must be inline"),
            };
            for section in required {
                assert!(
                    orchestrator.contains(section),
                    "prefab {} missing section {section}",
                    prefab.key()
                );
            }
        }
    }

    #[test]
    fn test_prefab_toml_templates_parse() {
        for prefab in Prefab::all() {
            let parsed = crate::definition::MobDefinition::from_toml(prefab.toml_template())
                .expect("prefab toml should parse");
            assert_eq!(
                parsed.id.as_str(),
                prefab.key(),
                "prefab toml id must match prefab key"
            );
        }
    }
}
