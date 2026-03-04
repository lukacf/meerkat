use std::collections::BTreeMap;

use indexmap::IndexMap;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::Pack;

pub struct AdvisorPack;

impl Pack for AdvisorPack {
    fn name(&self) -> &str {
        "advisor"
    }

    fn description(&self) -> &str {
        "Single agent, quick opinion — like asking a colleague"
    }

    fn agent_count(&self) -> usize {
        1
    }

    fn flow_step_count(&self) -> usize {
        1
    }

    fn definition(
        &self,
        task: &str,
        context: &str,
        model_overrides: &BTreeMap<String, String>,
    ) -> MobDefinition {
        let model = model_overrides
            .get("advisor")
            .cloned()
            .unwrap_or_else(|| "claude-sonnet-4-5".to_string());

        let context_block = if context.is_empty() {
            String::new()
        } else {
            format!("\n\n## Context\n\n{context}")
        };

        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("advisor"),
            Profile {
                model,
                skills: vec!["advisor-skill".to_string()],
                tools: ToolConfig::default(),
                peer_description: "Technical advisor".to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );

        let mut skills = BTreeMap::new();
        skills.insert(
            "advisor-skill".to_string(),
            SkillSource::Inline {
                content: include_str!("../../skills/advisor.md").to_string(),
            },
        );

        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("respond"),
            FlowStepSpec {
                role: ProfileName::from("advisor"),
                message: format!("{task}{context_block}"),
                depends_on: vec![],
                dispatch_mode: DispatchMode::default(),
                collection_policy: CollectionPolicy::default(),
                condition: None,
                timeout_ms: Some(120_000),
                expected_schema_ref: None,
                branch: None,
                depends_on_mode: DependencyMode::default(),
                allowed_tools: None,
                blocked_tools: None,
            },
        );

        let mut flows = BTreeMap::new();
        flows.insert(
            FlowId::from("main"),
            FlowSpec {
                description: Some("Single-agent advisory opinion".to_string()),
                steps,
            },
        );

        let mut profile_map = BTreeMap::new();
        profile_map.insert("advisor".to_string(), ProfileName::from("advisor"));

        MobDefinition {
            id: MobId::from(format!("force-advisor-{}", uuid::Uuid::new_v4().as_simple())),
            orchestrator: None,
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills,
            backend: BackendConfig::default(),
            flows,
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: Some(SpawnPolicyConfig::Auto { profile_map }),
            event_router: None,
        }
    }
}
