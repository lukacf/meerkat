//! CRUD tools for user-created mobs.
//!
//! User mobs are stored as mobpack archives (`.mobpack`) under
//! `.codemob-mcp/mobs/` in the project root. They are loaded dynamically
//! and available to `deliberate` without MCP restart.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use meerkat_core::types::ContentInput;
use meerkat_mob::definition::*;
use meerkat_mob::ids::*;
use meerkat_mob::profile::{Profile, ProfileBinding, ToolConfig};
use meerkat_mob::MobRuntimeMode;

use super::ToolCallError;
use crate::packs::{format_context, resolve_model, Pack};

// ── Storage path ─────────────────────────────────────────────────────────────

fn mobs_dir() -> PathBuf {
    PathBuf::from(".codemob-mcp/mobs")
}

fn mob_path(name: &str) -> PathBuf {
    mobs_dir().join(format!("{name}.json"))
}

fn validate_name(name: &str) -> Result<(), ToolCallError> {
    if name.is_empty() {
        return Err(ToolCallError::invalid_params("name must not be empty"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ToolCallError::invalid_params(
            "name must contain only alphanumeric, hyphen, and underscore characters",
        ));
    }
    Ok(())
}

// ── User mob config (JSON-serializable definition) ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMobConfig {
    pub name: String,
    pub description: String,
    /// "comms" is a legacy/default authoring shape that is normalized to a
    /// machine-owned flow; "flow" uses caller-supplied structured steps.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// For legacy comms mode: synthesis agent that receives peer flow outputs
    /// and produces the final result.
    pub orchestrator: Option<String>,
    pub agents: BTreeMap<String, UserAgentConfig>,
    /// Pairs of agent names to wire for explicit flow-mode comms ingress/egress.
    /// Ignored by legacy comms mode, which is normalized to strict flow edges.
    #[serde(default)]
    pub wiring: Vec<[String; 2]>,
    /// Flow definitions. Key is flow name (use "main").
    /// Only used when mode == "flow".
    #[serde(default)]
    pub flows: BTreeMap<String, Vec<UserFlowStep>>,
}

fn default_mode() -> String {
    "comms".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAgentConfig {
    pub model: String,
    /// System prompt / skill content for this agent.
    pub skill: String,
    #[serde(default)]
    pub peer_description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFlowStep {
    pub id: String,
    pub role: String,
    pub message: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    120_000
}

// ── UserMobConfig → Pack ─────────────────────────────────────────────────────

impl UserMobConfig {
    fn to_mob_definition(
        &self,
        task: &str,
        context: &str,
        model_overrides: &BTreeMap<String, String>,
        provider_params: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> MobDefinition {
        let ctx = format_context(context);
        let is_comms = self.mode == "comms";
        // `deliberate` no longer owns a local event-idle completion loop. Even
        // user-authored comms mobs therefore run as a MobMachine-owned flow;
        // the conversion is flow fan-in/synthesis, not peer-to-peer tool comms.
        let runtime = MobRuntimeMode::TurnDriven;
        let tools = ToolConfig {
            builtins: true,
            shell: true,
            comms: !is_comms,
            ..ToolConfig::default()
        };

        let mut profiles = BTreeMap::new();
        let mut skills = BTreeMap::new();

        for (name, agent) in &self.agents {
            let skill_key = format!("{name}-skill");
            profiles.insert(
                ProfileName::from(name.as_str()),
                ProfileBinding::Inline(Box::new(Profile {
                    model: resolve_model(model_overrides, name, &agent.model),
                    provider: None,
                    self_hosted_server_id: None,
                    image_generation_provider: None,
                    auto_compact_threshold: None,
                    resume_overrides: Vec::new(),
                    skills: vec![skill_key.clone()],
                    tools: tools.clone(),
                    peer_description: if agent.peer_description.is_empty() {
                        name.clone()
                    } else {
                        agent.peer_description.clone()
                    },
                    external_addressable: true,
                    backend: None,
                    runtime_mode: runtime.clone(),
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: provider_params.cloned(),
                })),
            );
            skills.insert(
                skill_key,
                SkillSource::Inline {
                    content: agent.skill.clone(),
                },
            );
        }

        let role_wiring: Vec<RoleWiringRule> = if is_comms {
            Vec::new()
        } else {
            self.wiring
                .iter()
                .map(|pair| RoleWiringRule {
                    a: ProfileName::from(pair[0].as_str()),
                    b: ProfileName::from(pair[1].as_str()),
                })
                .collect()
        };

        let has_orchestrator = self.orchestrator.is_some();
        let orchestrator = self.orchestrator.as_ref().map(|name| OrchestratorConfig {
            profile: ProfileName::from(name.as_str()),
        });

        let mut flows = BTreeMap::new();
        if is_comms {
            if let Some(orchestrator_name) = self
                .orchestrator
                .as_deref()
                .or_else(|| self.agents.keys().next().map(String::as_str))
            {
                let mut step_specs = indexmap::IndexMap::new();
                let peer_steps: Vec<(String, StepId)> = self
                    .agents
                    .keys()
                    .filter(|name| name.as_str() != orchestrator_name)
                    .enumerate()
                    .map(|(idx, name)| (name.clone(), StepId::from(format!("peer_{idx}"))))
                    .collect();
                for (agent_name, step_id) in &peer_steps {
                    step_specs.insert(
                        step_id.clone(),
                        FlowStepSpec {
                            role: ProfileName::from(agent_name.as_str()),
                            message: ContentInput::from(format!(
                                "Analyze the task independently. Return concise findings for the orchestrator to synthesize. Do not message peer agents directly.\n\n## Task\n{task}{ctx}"
                            )),
                            depends_on: Vec::new(),
                            dispatch_mode: DispatchMode::default(),
                            collection_policy: CollectionPolicy::default(),
                            condition: None,
                            timeout_ms: Some(default_timeout()),
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: DependencyMode::default(),
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: Some(StepOutputFormat::Text),
                        },
                    );
                }
                let peer_outputs = peer_steps
                    .iter()
                    .map(|(agent_name, step_id)| {
                        format!("### {agent_name}\n{{{{ steps.{step_id} }}}}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let message = if peer_outputs.is_empty() {
                    format!(
                        "Return the final answer for this task.\n\n## Task\n{task}{ctx}"
                    )
                } else {
                    format!(
                        "Synthesize the peer flow outputs into the final answer. Do not message peer agents directly.\n\n## Task\n{task}{ctx}\n\n## Peer outputs\n{peer_outputs}"
                    )
                };
                step_specs.insert(
                    StepId::from("orchestrate"),
                    FlowStepSpec {
                        role: ProfileName::from(orchestrator_name),
                        message: ContentInput::from(message),
                        depends_on: peer_steps
                            .iter()
                            .map(|(_, step_id)| step_id.clone())
                            .collect(),
                        dispatch_mode: DispatchMode::default(),
                        collection_policy: CollectionPolicy::default(),
                        condition: None,
                        timeout_ms: Some(default_timeout()),
                        expected_schema_ref: None,
                        branch: None,
                        depends_on_mode: DependencyMode::default(),
                        allowed_tools: None,
                        blocked_tools: None,
                        output_format: Some(StepOutputFormat::Text),
                    },
                );
                flows.insert(
                    FlowId::from("main"),
                    FlowSpec::new(Some("Comms-mode orchestrator flow".into()), step_specs, None),
                );
            }
        } else {
            for (flow_name, steps) in &self.flows {
                let mut step_specs = indexmap::IndexMap::new();
                for step in steps {
                    let msg = step
                        .message
                        .replace("{{ task }}", &format!("{task}{ctx}"))
                        .replace("{{task}}", &format!("{task}{ctx}"));
                    step_specs.insert(
                        StepId::from(step.id.as_str()),
                        FlowStepSpec {
                            role: ProfileName::from(step.role.as_str()),
                            message: ContentInput::from(msg),
                            depends_on: step
                                .depends_on
                                .iter()
                                .map(|s| StepId::from(s.as_str()))
                                .collect(),
                            dispatch_mode: DispatchMode::default(),
                            collection_policy: CollectionPolicy::default(),
                            condition: None,
                            timeout_ms: Some(step.timeout_ms),
                            expected_schema_ref: None,
                            branch: None,
                            depends_on_mode: DependencyMode::default(),
                            allowed_tools: None,
                            blocked_tools: None,
                            output_format: Some(StepOutputFormat::Text),
                        },
                    );
                }
                flows.insert(
                    FlowId::from(flow_name.as_str()),
                    FlowSpec::new(None, step_specs, None),
                );
            }
        }

        let mut definition = MobDefinition::explicit(MobId::from(format!(
            "codemob-user-{}-{}",
            self.name,
            uuid::Uuid::new_v4().as_simple()
        )));
        definition.orchestrator = orchestrator;
        definition.profiles = profiles;
        definition.wiring = WiringRules {
            auto_wire_orchestrator: has_orchestrator,
            role_wiring,
        };
        definition.skills = skills;
        definition.flows = flows;
        definition
    }
}

/// Adapter: wraps a UserMobConfig as a Pack for the registry.
pub struct UserPack {
    config: UserMobConfig,
}

impl UserPack {
    pub fn new(config: UserMobConfig) -> Self {
        Self { config }
    }
}

impl Pack for UserPack {
    fn name(&self) -> &str {
        &self.config.name
    }
    fn description(&self) -> &str {
        &self.config.description
    }
    fn agent_count(&self) -> usize {
        self.config.agents.len()
    }
    fn flow_step_count(&self) -> usize {
        if self.config.mode == "flow" {
            self.config.flows.values().map(|steps| steps.len()).sum()
        } else if self.config.agents.is_empty() {
            0
        } else {
            self.config.agents.len()
        }
    }
    fn definition(
        &self,
        task: &str,
        context: &str,
        model_overrides: &BTreeMap<String, String>,
        provider_params: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> MobDefinition {
        self.config
            .to_mob_definition(task, context, model_overrides, provider_params)
    }
}

// ── CRUD handlers ────────────────────────────────────────────────────────────

pub async fn handle_create(arguments: &Value) -> Result<Value, ToolCallError> {
    let config: UserMobConfig = serde_json::from_value(
        arguments
            .get("definition")
            .cloned()
            .unwrap_or(arguments.clone()),
    )
    .map_err(|e| ToolCallError::invalid_params(format!("Invalid mob definition: {e}")))?;

    validate_name(&config.name)?;

    if config.agents.is_empty() {
        return Err(ToolCallError::invalid_params(
            "At least one agent is required",
        ));
    }

    let dir = mobs_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| ToolCallError::internal(format!("Failed to create mobs directory: {e}")))?;

    let path = mob_path(&config.name);
    if path.exists() {
        return Err(ToolCallError::invalid_params(format!(
            "Mob '{}' already exists. Use update_mob to modify it.",
            config.name
        )));
    }

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| ToolCallError::internal(format!("Serialization failed: {e}")))?;
    std::fs::write(&path, &json)
        .map_err(|e| ToolCallError::internal(format!("Failed to write mob file: {e}")))?;

    Ok(
        json!({"content": [{"type": "text", "text": format!("Created mob '{}' with {} agent(s). Saved to {}", config.name, config.agents.len(), path.display())}]}),
    )
}

pub async fn handle_get(arguments: &Value) -> Result<Value, ToolCallError> {
    let name = arguments
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolCallError::invalid_params("Missing 'name' parameter"))?;
    validate_name(name)?;

    let path = mob_path(name);
    let json = std::fs::read_to_string(&path)
        .map_err(|_| ToolCallError::invalid_params(format!("Mob '{name}' not found")))?;

    Ok(json!({"content": [{"type": "text", "text": json}]}))
}

pub async fn handle_update(arguments: &Value) -> Result<Value, ToolCallError> {
    let config: UserMobConfig = serde_json::from_value(
        arguments
            .get("definition")
            .cloned()
            .unwrap_or(arguments.clone()),
    )
    .map_err(|e| ToolCallError::invalid_params(format!("Invalid mob definition: {e}")))?;

    validate_name(&config.name)?;

    let path = mob_path(&config.name);
    if !path.exists() {
        return Err(ToolCallError::invalid_params(format!(
            "Mob '{}' does not exist. Use create_mob first.",
            config.name
        )));
    }

    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| ToolCallError::internal(format!("Serialization failed: {e}")))?;
    std::fs::write(&path, &json)
        .map_err(|e| ToolCallError::internal(format!("Failed to write mob file: {e}")))?;

    Ok(json!({"content": [{"type": "text", "text": format!("Updated mob '{}'", config.name)}]}))
}

pub async fn handle_delete(arguments: &Value) -> Result<Value, ToolCallError> {
    let name = arguments
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolCallError::invalid_params("Missing 'name' parameter"))?;
    validate_name(name)?;

    let path = mob_path(name);
    if !path.exists() {
        return Err(ToolCallError::invalid_params(format!(
            "Mob '{name}' not found"
        )));
    }

    std::fs::remove_file(&path)
        .map_err(|e| ToolCallError::internal(format!("Failed to delete mob file: {e}")))?;

    Ok(json!({"content": [{"type": "text", "text": format!("Deleted mob '{name}'")}]}))
}

/// Load all user mobs from disk and register them in the pack registry.
pub fn load_user_packs(dir: &Path) -> Vec<Box<dyn Pack>> {
    let mut packs: Vec<Box<dyn Pack>> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return packs,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read user mob");
                continue;
            }
        };
        let config: UserMobConfig = match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to parse user mob");
                continue;
            }
        };
        packs.push(Box::new(UserPack::new(config)));
    }
    packs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_user_mob(mode: &str) -> UserMobConfig {
        let mut agents = BTreeMap::new();
        agents.insert(
            "lead".to_string(),
            UserAgentConfig {
                model: "gpt-5.5".to_string(),
                skill: "You lead the mob.".to_string(),
                peer_description: "Lead".to_string(),
            },
        );
        agents.insert(
            "critic".to_string(),
            UserAgentConfig {
                model: "claude-sonnet-4-6".to_string(),
                skill: "You critique plans.".to_string(),
                peer_description: "Critic".to_string(),
            },
        );
        UserMobConfig {
            name: "custom".to_string(),
            description: "Custom mob".to_string(),
            mode: mode.to_string(),
            orchestrator: Some("lead".to_string()),
            agents,
            wiring: vec![["lead".to_string(), "critic".to_string()]],
            flows: BTreeMap::new(),
        }
    }

    #[test]
    fn comms_user_pack_synthesizes_machine_owned_main_flow() {
        let pack = UserPack::new(sample_user_mob("comms"));

        assert_eq!(pack.flow_step_count(), 2);

        let definition = pack.definition("Ship it", "extra context", &BTreeMap::new(), None);
        let main = definition
            .flows
            .get(&FlowId::from("main"))
            .expect("comms user packs synthesize a main flow");
        let peer = main
            .steps
            .get(&StepId::from("peer_0"))
            .expect("synthetic peer analysis step");
        let orchestrate = main
            .steps
            .get(&StepId::from("orchestrate"))
            .expect("synthetic orchestrator step");

        assert_eq!(peer.role, ProfileName::from("critic"));
        assert!(peer.depends_on.is_empty());
        assert_eq!(orchestrate.role, ProfileName::from("lead"));
        assert_eq!(orchestrate.depends_on, vec![StepId::from("peer_0")]);
        let text = orchestrate.message.text_content();
        assert!(
            text.contains("Ship it")
                && text.contains("extra context")
                && text.contains("{{ steps.peer_0 }}")
                && text.contains("Do not message peer agents directly")
        );
        assert!(definition.wiring.role_wiring.is_empty());
        for binding in definition.profiles.values() {
            let profile = binding.as_inline().expect("inline user profile");
            assert_eq!(profile.runtime_mode, MobRuntimeMode::TurnDriven);
            assert!(!profile.tools.comms);
        }
    }
}
