use std::collections::BTreeMap;

use crate::definition::{MobDefinition, OrchestratorConfig, SkillSource, WiringRules};
use crate::ids::{MobId, ProfileName};
use crate::profile::{Profile, ToolConfig};

pub enum Prefab {
    CodingSwarm,
    CodeReview,
    ResearchTeam,
    Pipeline,
}

pub struct PrefabInfo {
    pub name: &'static str,
    pub description: &'static str,
}

impl Prefab {
    pub fn list() -> Vec<PrefabInfo> {
        vec![
            PrefabInfo {
                name: "coding-swarm",
                description: "Spawns developers and coordinates via task board",
            },
            PrefabInfo {
                name: "code-review",
                description: "Spawns reviewers and aggregates feedback",
            },
            PrefabInfo {
                name: "research-team",
                description: "Spawns researchers, then synthesizer",
            },
            PrefabInfo {
                name: "pipeline",
                description: "Spawns a simple staged pipeline",
            },
        ]
    }

    pub fn definition(&self) -> MobDefinition {
        match self {
            Self::CodingSwarm => coding_swarm(),
            Self::CodeReview => code_review(),
            Self::ResearchTeam => research_team(),
            Self::Pipeline => pipeline(),
        }
    }
}

fn coding_swarm() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    let orchestrator_profile = ProfileName::from("coordinator");
    profiles.insert(
        orchestrator_profile.clone(),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["swarm_leader".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: true,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "coding coordinator".to_string(),
            external_addressable: true,
        },
    );
    profiles.insert(
        ProfileName::from("developer"),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["developer".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: false,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "worker developer".to_string(),
            external_addressable: true,
        },
    );

    let skills = [
        (
            "swarm_leader".to_string(),
            SkillSource::Inline(
                "# Role\nCoordinate a coding squad.\n\n# Available Tools\nUse mob, mob_tasks, comms, memory, and builtins to assign and track work.\n\n# Communication\nSend concise task briefs, status checks, and unblock requests.\n\n# Coordination Pattern\nBreak work into tasks, assign workers, monitor progress, and merge outcomes.\n\n# Worker Lifecycle\nSpawn or engage workers, provide scoped tasks, review outputs, and release workers when complete.\n\n# Completion\nFinish when all requested coding outcomes are delivered and summarized.\n"
                    .to_string(),
            ),
        ),
        ("developer".to_string(), SkillSource::Inline("# Role\nImplement requested code changes\n".to_string())),
    ]
    .into_iter()
    .collect();

    MobDefinition {
        id: MobId::from("coding-swarm"),
        orchestrator: Some(OrchestratorConfig {
            profile: orchestrator_profile,
            startup_message: Some("Coordinate task board for coding job".to_string()),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: Vec::new(),
        },
        skills,
    }
}

fn code_review() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    let orchestrator_profile = ProfileName::from("review_lead");
    profiles.insert(
        orchestrator_profile.clone(),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["review_lead".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: true,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "review lead".to_string(),
            external_addressable: true,
        },
    );

    profiles.insert(
        ProfileName::from("reviewer"),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["reviewer".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: false,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "human-like reviewer".to_string(),
            external_addressable: true,
        },
    );

    let skills = [
        (
            "review_lead".to_string(),
            SkillSource::Inline(
                "# Role\nOrchestrate review flow.\n\n# Available Tools\nUse mob, mob_tasks, comms, memory, and builtins to coordinate reviewer activity.\n\n# Communication\nShare review scope, code context, deadlines, and decision updates.\n\n# Coordination Pattern\nSplit review areas, assign reviewers, aggregate findings, and resolve conflicts.\n\n# Worker Lifecycle\nEngage reviewers, collect findings, request follow-ups, and close reviewer tasks.\n\n# Completion\nFinish when findings are consolidated and final review guidance is delivered.\n"
                    .to_string(),
            ),
        ),
        (
            "reviewer".to_string(),
            SkillSource::Inline("# Role\nReview code thoroughly\n".to_string()),
        ),
    ]
    .into_iter()
    .collect();

    MobDefinition {
        id: MobId::from("code-review"),
        orchestrator: Some(OrchestratorConfig {
            profile: orchestrator_profile,
            startup_message: Some("Coordinate code review".to_string()),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: Vec::new(),
        },
        skills,
    }
}

fn research_team() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    let orchestrator_profile = ProfileName::from("research_lead");
    profiles.insert(
        orchestrator_profile.clone(),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["research_lead".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: true,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "research lead".to_string(),
            external_addressable: true,
        },
    );

    MobDefinition {
        id: MobId::from("research-team"),
        orchestrator: Some(OrchestratorConfig {
            profile: orchestrator_profile,
            startup_message: Some("Coordinate research and synthesis".to_string()),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: Vec::new(),
        },
        skills: [
            (
                "research_lead".to_string(),
                SkillSource::Inline(
                    "# Role\nCoordinate research swarm.\n\n# Available Tools\nUse mob, mob_tasks, comms, memory, and builtins to run research assignments.\n\n# Communication\nProvide clear research questions, source quality expectations, and synthesis goals.\n\n# Coordination Pattern\nDistribute research threads, gather evidence, and drive synthesis into conclusions.\n\n# Worker Lifecycle\nAssign researchers, gather intermediate results, trigger synthesis, and close completed work.\n\n# Completion\nFinish when research findings and synthesized conclusions are delivered.\n"
                        .to_string(),
                ),
            ),
            (
                "researcher".to_string(),
                SkillSource::Inline("# Role\nResearch topic\n".to_string()),
            ),
            (
                "synthesizer".to_string(),
                SkillSource::Inline("# Role\nSynthesize findings\n".to_string()),
            ),
        ]
        .into_iter()
        .collect(),
    }
}

fn pipeline() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    let orchestrator_profile = ProfileName::from("pipeline_orchestrator");
    profiles.insert(
        orchestrator_profile.clone(),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["pipeline".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: true,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "pipeline coordinator".to_string(),
            external_addressable: true,
        },
    );

    MobDefinition {
        id: MobId::from("pipeline"),
        orchestrator: Some(OrchestratorConfig {
            profile: orchestrator_profile,
            startup_message: Some("Coordinate pipeline stages".to_string()),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: Vec::new(),
        },
        skills: [(
            "pipeline".to_string(),
            SkillSource::Inline(
                "# Role\nCoordinate sequential stages.\n\n# Available Tools\nUse mob, mob_tasks, comms, memory, and builtins to manage stage execution.\n\n# Communication\nIssue stage instructions, handoff notes, and escalation messages.\n\n# Coordination Pattern\nRun stages in order, verify each handoff, and track pipeline state.\n\n# Worker Lifecycle\nStart stage workers, monitor completion, trigger next stage, and close finished workers.\n\n# Completion\nFinish when all stages complete and final pipeline output is reported.\n"
                    .to_string(),
            ),
        )]
        .into_iter()
        .collect(),
    }
}
