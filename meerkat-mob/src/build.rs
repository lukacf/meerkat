//! Profile to AgentBuildConfig compilation.
//!
//! Maps a mob [`Profile`] to an [`AgentBuildConfig`] with the correct
//! flags for host-mode operation, comms naming, peer metadata, and
//! tool overrides. Bridges to [`CreateSessionRequest`] for session creation.

use crate::definition::{MobDefinition, SkillSource};
use crate::error::MobError;
use crate::ids::{MeerkatId, MobId, ProfileName};
use crate::profile::Profile;
use meerkat::AgentBuildConfig;
use meerkat_core::PeerMeta;
use meerkat_core::service::CreateSessionRequest;
use std::sync::Arc;

/// Build an [`AgentBuildConfig`] from a mob profile.
///
/// This is the first step in the construction chain:
///   Profile -> `build_agent_config()` -> `to_create_session_request()` -> `SessionService::create_session()`
///
/// All meerkats are created with `host_mode=true` so they stay alive
/// for comms after the initial prompt completes.
pub fn build_agent_config(
    mob_id: &MobId,
    profile_name: &ProfileName,
    meerkat_id: &MeerkatId,
    profile: &Profile,
    definition: &MobDefinition,
    external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
) -> Result<AgentBuildConfig, MobError> {
    if !profile.tools.comms {
        return Err(MobError::WiringError(format!(
            "profile '{profile_name}' has tools.comms=false; mob meerkats require comms=true"
        )));
    }

    // Comms name: "{mob_id}/{profile}/{meerkat_id}"
    let comms_name = format!("{}/{}/{}", mob_id, profile_name, meerkat_id);

    // Peer metadata with labels for discovery
    let peer_meta = PeerMeta::default()
        .with_description(&profile.peer_description)
        .with_label("mob_id", mob_id.as_str())
        .with_label("role", profile_name.as_str())
        .with_label("meerkat_id", meerkat_id.as_str());

    // Realm ID for namespace isolation
    let realm_id = format!("mob:{}", mob_id);

    // Assemble system prompt from skills + mob comms instructions
    let system_prompt = assemble_system_prompt(profile, definition)?;

    let mut config = AgentBuildConfig::new(profile.model.clone());
    config.host_mode = true;
    config.comms_name = Some(comms_name);
    config.peer_meta = Some(peer_meta);
    config.realm_id = Some(realm_id);
    config.system_prompt = Some(system_prompt);

    // Map ToolConfig booleans to override flags
    config.override_builtins = Some(profile.tools.builtins);
    config.override_shell = Some(profile.tools.shell);
    config.override_memory = Some(profile.tools.memory);

    // Sub-agents are disabled for mob meerkats (they use comms instead)
    config.override_subagents = Some(false);

    // External tools (mob tools, task tools, rust bundles composed externally)
    config.external_tools = external_tools;

    Ok(config)
}

/// Bridge an [`AgentBuildConfig`] to a [`CreateSessionRequest`].
///
/// This is the second step: the config is converted to the service-level
/// request type that `SessionService::create_session()` accepts.
pub fn to_create_session_request(
    config: &AgentBuildConfig,
    prompt: String,
) -> CreateSessionRequest {
    let build_options = config.to_session_build_options();

    CreateSessionRequest {
        model: config.model.clone(),
        prompt,
        system_prompt: config.system_prompt.clone(),
        max_tokens: config.max_tokens,
        event_tx: None,
        host_mode: config.host_mode,
        skill_references: None,
        build: Some(build_options),
    }
}

/// Assemble the system prompt for a mob meerkat from skills and mob comms instructions.
fn assemble_system_prompt(profile: &Profile, definition: &MobDefinition) -> Result<String, MobError> {
    let mut sections = Vec::new();

    // Resolve inline skills from the definition
    for skill_ref in &profile.skills {
        if let Some(source) = definition.skills.get(skill_ref) {
            match source {
                SkillSource::Inline { content } => {
                    sections.push(content.clone());
                }
                SkillSource::Path { path } => {
                    let content = std::fs::read_to_string(path).map_err(|error| {
                        MobError::Internal(format!(
                            "failed to read skill file '{path}' while building system prompt: {error}"
                        ))
                    })?;
                    sections.push(content);
                }
            }
        }
    }

    // Add mob comms instructions
    sections.push(MOB_COMMS_INSTRUCTIONS.to_string());

    Ok(sections.join("\n\n"))
}

/// Standard instructions injected into every mob meerkat's system prompt
/// explaining how to communicate with peers via comms.
const MOB_COMMS_INSTRUCTIONS: &str = "\
## Mob Communication

You are a meerkat (agent) in a collaborative mob. You communicate with \
other meerkats via the comms system:

- Use `peers()` to discover other meerkats you are wired to.
- Send requests via PeerRequest with an intent string and JSON params.
- Respond to incoming PeerRequests with PeerResponse.
- You will receive notifications when peers are added (mob.peer_added) \
or removed (mob.peer_retired).
- Handle mob.peer_added by acknowledging the new peer.
- Handle mob.peer_retired by removing the peer from your working set.";

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::definition::{MobDefinition, OrchestratorConfig, WiringRules};
    use crate::profile::ToolConfig;
    use std::collections::BTreeMap;
    use std::fs;

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            Profile {
                model: "claude-opus-4-6".into(),
                skills: vec!["leader-skill".into()],
                tools: ToolConfig {
                    builtins: true,
                    shell: true,
                    comms: true,
                    memory: false,
                    mob: true,
                    mob_tasks: true,
                    mcp: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "Orchestrates the mob".into(),
                external_addressable: true,
            },
        );
        profiles.insert(
            ProfileName::from("worker"),
            Profile {
                model: "claude-sonnet-4-5".into(),
                skills: vec![],
                tools: ToolConfig {
                    builtins: true,
                    shell: false,
                    comms: true,
                    memory: false,
                    mob: false,
                    mob_tasks: false,
                    mcp: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "Does work".into(),
                external_addressable: false,
            },
        );

        let mut skills = BTreeMap::new();
        skills.insert(
            "leader-skill".into(),
            SkillSource::Inline {
                content: "You are the team lead.".into(),
            },
        );

        MobDefinition {
            id: MobId::from("test-mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills,
        }
    }

    #[test]
    fn test_build_agent_config_host_mode() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            profile,
            &def,
            None,
        )
        .expect("build_agent_config");

        assert!(config.host_mode, "host_mode must be true");
    }

    #[test]
    fn test_build_agent_config_comms_name() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            profile,
            &def,
            None,
        )
        .expect("build_agent_config");

        assert_eq!(
            config.comms_name.as_deref(),
            Some("test-mob/lead/lead-1"),
            "comms_name should be mob_id/profile/meerkat_id"
        );
    }

    #[test]
    fn test_build_agent_config_peer_meta_labels() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("worker")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("worker"),
            &MeerkatId::from("w-1"),
            profile,
            &def,
            None,
        )
        .expect("build_agent_config");

        let meta = config.peer_meta.as_ref().expect("peer_meta should be set");
        assert_eq!(
            meta.labels.get("mob_id").map(String::as_str),
            Some("test-mob")
        );
        assert_eq!(meta.labels.get("role").map(String::as_str), Some("worker"));
        assert_eq!(
            meta.labels.get("meerkat_id").map(String::as_str),
            Some("w-1")
        );
        assert_eq!(meta.description.as_deref(), Some("Does work"));
    }

    #[test]
    fn test_build_agent_config_realm_id() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            profile,
            &def,
            None,
        )
        .expect("build_agent_config");

        assert_eq!(
            config.realm_id.as_deref(),
            Some("mob:test-mob"),
            "realm_id should be 'mob:test-mob'"
        );
    }

    #[test]
    fn test_build_agent_config_tool_overrides() {
        let def = sample_definition();

        // Lead profile has builtins=true, shell=true, memory=false
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            lead,
            &def,
            None,
        )
        .expect("build_agent_config");
        assert_eq!(config.override_builtins, Some(true));
        assert_eq!(config.override_shell, Some(true));
        assert_eq!(config.override_memory, Some(false));
        assert_eq!(config.override_subagents, Some(false));

        // Worker profile has builtins=true, shell=false, memory=false
        let worker = &def.profiles[&ProfileName::from("worker")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("worker"),
            &MeerkatId::from("w-1"),
            worker,
            &def,
            None,
        )
        .expect("build_agent_config");
        assert_eq!(config.override_builtins, Some(true));
        assert_eq!(config.override_shell, Some(false));
        assert_eq!(config.override_memory, Some(false));
    }

    #[test]
    fn test_build_agent_config_fails_when_comms_disabled() {
        let mut def = sample_definition();
        let worker = def
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile");
        worker.tools.comms = false;
        let worker = def
            .profiles
            .get(&ProfileName::from("worker"))
            .expect("worker profile");

        let result = build_agent_config(
            &def.id,
            &ProfileName::from("worker"),
            &MeerkatId::from("w-1"),
            worker,
            &def,
            None,
        );
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "tools.comms=false must be rejected at build_agent_config"
        );
    }

    #[test]
    fn test_build_agent_config_system_prompt_includes_skills() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            lead,
            &def,
            None,
        )
        .expect("build_agent_config");

        let prompt = config.system_prompt.as_deref().expect("system_prompt set");
        assert!(
            prompt.contains("You are the team lead."),
            "prompt should contain resolved inline skill"
        );
        assert!(
            prompt.contains("Mob Communication"),
            "prompt should contain mob comms instructions"
        );
    }

    #[test]
    fn test_build_agent_config_model() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            lead,
            &def,
            None,
        )
        .expect("build_agent_config");

        assert_eq!(config.model, "claude-opus-4-6");
    }

    #[test]
    fn test_to_create_session_request() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            lead,
            &def,
            None,
        )
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Hello mob".into());
        assert_eq!(req.model, "claude-opus-4-6");
        assert!(req.host_mode, "host_mode must carry through");
        assert_eq!(req.prompt, "Hello mob");
        assert!(req.system_prompt.is_some());

        let build = req.build.expect("build options should be set");
        assert_eq!(build.comms_name.as_deref(), Some("test-mob/lead/lead-1"));
        assert!(build.peer_meta.is_some());
        assert_eq!(build.realm_id.as_deref(), Some("mob:test-mob"));
        assert_eq!(build.override_builtins, Some(true));
        assert_eq!(build.override_shell, Some(true));
        assert_eq!(build.override_subagents, Some(false));
    }

    #[test]
    fn test_to_create_session_request_worker() {
        let def = sample_definition();
        let worker = &def.profiles[&ProfileName::from("worker")];
        let config = build_agent_config(
            &def.id,
            &ProfileName::from("worker"),
            &MeerkatId::from("w-1"),
            worker,
            &def,
            None,
        )
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Start working".into());
        assert_eq!(req.model, "claude-sonnet-4-5");
        assert!(req.host_mode);
        let build = req.build.expect("build options");
        assert_eq!(build.override_shell, Some(false));
    }

    #[test]
    fn test_build_agent_config_resolves_path_skills() {
        let mut def = sample_definition();
        let tempdir = tempfile::tempdir().expect("tempdir");
        let skill_path = tempdir.path().join("leader.md");
        fs::write(&skill_path, "Path skill content for leader.").expect("write path skill");

        def.skills.insert(
            "path-skill".into(),
            SkillSource::Path {
                path: skill_path.display().to_string(),
            },
        );
        let lead = def
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("lead profile");
        lead.skills.push("path-skill".into());
        let lead = def
            .profiles
            .get(&ProfileName::from("lead"))
            .expect("lead profile");

        let config = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            lead,
            &def,
            None,
        )
        .expect("build_agent_config should resolve path skill");

        let prompt = config.system_prompt.expect("system prompt");
        assert!(prompt.contains("Path skill content for leader."));
        assert!(!prompt.contains("[skill from:"));
    }

    #[test]
    fn test_build_agent_config_fails_for_missing_path_skill() {
        let mut def = sample_definition();
        let missing_path = std::env::temp_dir()
            .join("meerkat-mob-missing-skill.md")
            .display()
            .to_string();
        def.skills.insert(
            "missing-path-skill".into(),
            SkillSource::Path {
                path: missing_path.clone(),
            },
        );
        let lead = def
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("lead profile");
        lead.skills = vec!["missing-path-skill".into()];
        let lead = def
            .profiles
            .get(&ProfileName::from("lead"))
            .expect("lead profile");

        let err = build_agent_config(
            &def.id,
            &ProfileName::from("lead"),
            &MeerkatId::from("lead-1"),
            lead,
            &def,
            None,
        )
        .expect_err("missing path skill should fail");

        match err {
            MobError::Internal(message) => {
                assert!(message.contains("failed to read skill file"));
                assert!(message.contains(&missing_path));
            }
            other => panic!("expected MobError::Internal, got: {other:?}"),
        }
    }
}
