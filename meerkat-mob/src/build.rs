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

/// Parameters for building an agent config from a mob profile.
pub struct BuildAgentConfigParams<'a> {
    pub mob_id: &'a MobId,
    pub profile_name: &'a ProfileName,
    pub meerkat_id: &'a MeerkatId,
    pub profile: &'a Profile,
    pub definition: &'a MobDefinition,
    pub external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    pub context: Option<serde_json::Value>,
    pub labels: Option<std::collections::BTreeMap<String, String>>,
}

/// Build an [`AgentBuildConfig`] from a mob profile.
///
/// This is the first step in the construction chain:
///   Profile -> `build_agent_config()` -> `to_create_session_request()` -> `SessionService::create_session()`
///
/// Mob-managed sessions are created with `host_mode=false` so spawn returns
/// promptly from nested tool dispatch paths. Lifecycles are managed explicitly
/// by mob runtime commands (`start_turn`, `retire`, etc.).
pub async fn build_agent_config(
    params: BuildAgentConfigParams<'_>,
) -> Result<AgentBuildConfig, MobError> {
    let BuildAgentConfigParams {
        mob_id,
        profile_name,
        meerkat_id,
        profile,
        definition,
        external_tools,
        context,
        labels,
    } = params;

    if !profile.tools.comms {
        return Err(MobError::WiringError(format!(
            "profile '{profile_name}' has tools.comms=false; mob meerkats require comms=true"
        )));
    }

    // Comms name: "{mob_id}/{profile}/{meerkat_id}"
    let comms_name = format!("{mob_id}/{profile_name}/{meerkat_id}");

    // Peer metadata with labels for discovery.
    // Application labels are applied first, then mob standard labels
    // overwrite on conflict.
    let mut peer_meta = PeerMeta::default().with_description(&profile.peer_description);
    if let Some(app_labels) = labels {
        for (k, v) in app_labels {
            peer_meta = peer_meta.with_label(&k, &v);
        }
    }
    // Mob standard labels overwrite app labels on conflict
    peer_meta = peer_meta
        .with_label("mob_id", mob_id.as_str())
        .with_label("role", profile_name.as_str())
        .with_label("meerkat_id", meerkat_id.as_str());

    // Realm ID for namespace isolation
    let realm_id = format!("mob:{mob_id}");

    // Assemble system prompt from profile skills (inline/path-based).
    let system_prompt = assemble_system_prompt(profile, definition).await?;

    let mut config = AgentBuildConfig::new(profile.model.clone());
    config.host_mode = false;
    config.comms_name = Some(comms_name);
    config.peer_meta = Some(peer_meta);
    config.realm_id = Some(realm_id);
    if !system_prompt.is_empty() {
        config.system_prompt = Some(system_prompt);
    }

    // Mob comms instructions are delivered as an embedded skill via
    // preload_skills. The `skills` feature is required on the meerkat
    // dependency (enforced in Cargo.toml) so the skill engine is
    // always available. Skills are appended as extra_sections in
    // prompt assembly, which survives per-request system_prompt
    // overrides.
    config.preload_skills = Some(vec![meerkat_core::skills::SkillId::from(
        "mob-communication",
    )]);

    // Silent comms intents: peer lifecycle notifications are injected
    // into context without triggering an LLM turn.
    config.silent_comms_intents = vec!["mob.peer_added".into(), "mob.peer_retired".into()];
    config.max_inline_peer_notifications = profile.max_inline_peer_notifications;

    // Map ToolConfig booleans to override flags
    config.override_builtins = Some(profile.tools.builtins);
    config.override_shell = Some(profile.tools.shell);
    config.override_memory = Some(profile.tools.memory);

    // Sub-agents are disabled for mob meerkats (they use comms instead)
    config.override_subagents = Some(false);

    // External tools (mob tools, task tools, rust bundles composed externally)
    config.external_tools = external_tools;

    // Opaque application context passed through to the agent build pipeline
    config.app_context = context;

    // Structured output: convert JSON schema value to OutputSchema
    if let Some(schema_value) = &profile.output_schema {
        let schema = meerkat_core::MeerkatSchema::new(schema_value.clone()).map_err(|e| {
            MobError::WiringError(format!(
                "invalid output_schema for profile '{profile_name}': {e}"
            ))
        })?;
        config.output_schema = Some(meerkat_core::OutputSchema {
            schema,
            name: Some(format!("{mob_id}_{profile_name}")),
            strict: true,
            compat: Default::default(),
            format: Default::default(),
        });
    }

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
        // Mob runtime owns lifecycle startup and starts autonomous host loops
        // explicitly after provisioning. Avoid synchronous first-turn execution
        // during create_session so spawn does not block on LLM latency.
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        build: Some(build_options),
        labels: None,
    }
}

/// Assemble the system prompt for a mob meerkat from profile-defined skills.
///
/// Mob comms instructions are loaded separately as an embedded skill via
/// `preload_skills` in `build_agent_config()` — not assembled here.
async fn assemble_system_prompt(
    profile: &Profile,
    definition: &MobDefinition,
) -> Result<String, MobError> {
    let mut sections = Vec::new();

    // Resolve inline skills from the definition
    for skill_ref in &profile.skills {
        if let Some(source) = definition.skills.get(skill_ref) {
            match source {
                SkillSource::Inline { content } => {
                    sections.push(content.clone());
                }
                SkillSource::Path { path } => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let content = tokio::fs::read_to_string(path).await.map_err(|error| {
                            MobError::Internal(format!(
                                "failed to read skill file '{path}' while building system prompt: {error}"
                            ))
                        })?;
                        sections.push(content);
                    }
                    #[cfg(target_arch = "wasm32")]
                    return Err(MobError::Internal(format!(
                        "file-based skill path '{path}' is not supported on wasm32"
                    )));
                }
            }
        }
    }

    Ok(sections.join("\n\n"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, MobDefinition, OrchestratorConfig, WiringRules};
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
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
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
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
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
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
        }
    }

    #[tokio::test]
    async fn test_build_agent_config_non_host_mode() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert!(!config.host_mode, "host_mode must be false for mob spawn");
    }

    #[tokio::test]
    async fn test_build_agent_config_comms_name() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.comms_name.as_deref(),
            Some("test-mob/lead/lead-1"),
            "comms_name should be mob_id/profile/meerkat_id"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_peer_meta_labels() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("worker")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            meerkat_id: &MeerkatId::from("w-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
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

    #[tokio::test]
    async fn test_build_agent_config_realm_id() {
        let def = sample_definition();
        let profile = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.realm_id.as_deref(),
            Some("mob:test-mob"),
            "realm_id should be 'mob:test-mob'"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_tool_overrides() {
        let def = sample_definition();

        // Lead profile has builtins=true, shell=true, memory=false
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");
        assert_eq!(config.override_builtins, Some(true));
        assert_eq!(config.override_shell, Some(true));
        assert_eq!(config.override_memory, Some(false));
        assert_eq!(config.override_subagents, Some(false));

        // Worker profile has builtins=true, shell=false, memory=false
        let worker = &def.profiles[&ProfileName::from("worker")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            meerkat_id: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");
        assert_eq!(config.override_builtins, Some(true));
        assert_eq!(config.override_shell, Some(false));
        assert_eq!(config.override_memory, Some(false));
    }

    #[tokio::test]
    async fn test_build_agent_config_fails_when_comms_disabled() {
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

        let result = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            meerkat_id: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "tools.comms=false must be rejected at build_agent_config"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_system_prompt_includes_skills() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        let prompt = config.system_prompt.as_deref().expect("system_prompt set");
        assert!(
            prompt.contains("You are the team lead."),
            "prompt should contain resolved inline skill"
        );
        // Mob comms instructions are delivered via preload_skills (embedded
        // skill), not baked into system_prompt — verified in the separate
        // test_build_agent_config_preloads_mob_communication_skill test.
    }

    #[tokio::test]
    async fn test_build_agent_config_preloads_mob_communication_skill() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        let preload = config
            .preload_skills
            .as_ref()
            .expect("preload_skills should be set");
        assert!(
            preload.iter().any(|id| id.0 == "mob-communication"),
            "preload_skills should include mob-communication"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_sets_silent_comms_intents() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert!(
            config
                .silent_comms_intents
                .contains(&"mob.peer_added".to_string()),
            "silent_comms_intents should include mob.peer_added"
        );
        assert!(
            config
                .silent_comms_intents
                .contains(&"mob.peer_retired".to_string()),
            "silent_comms_intents should include mob.peer_retired"
        );
        assert_eq!(config.max_inline_peer_notifications, None);
    }

    #[tokio::test]
    async fn test_build_agent_config_propagates_max_inline_peer_notifications() {
        let mut def = sample_definition();
        let lead_key = ProfileName::from("lead");
        def.profiles
            .get_mut(&lead_key)
            .expect("lead profile")
            .max_inline_peer_notifications = Some(15);
        let lead = &def.profiles[&lead_key];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &lead_key,
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.max_inline_peer_notifications, Some(15));
    }

    #[tokio::test]
    async fn test_build_agent_config_model() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.model, "claude-opus-4-6");
    }

    #[tokio::test]
    async fn test_to_create_session_request() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Hello mob".into());
        assert_eq!(req.model, "claude-opus-4-6");
        assert!(!req.host_mode, "host_mode must carry through as false");
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

    #[tokio::test]
    async fn test_to_create_session_request_worker() {
        let def = sample_definition();
        let worker = &def.profiles[&ProfileName::from("worker")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            meerkat_id: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Start working".into());
        assert_eq!(req.model, "claude-sonnet-4-5");
        assert!(!req.host_mode);
        let build = req.build.expect("build options");
        assert_eq!(build.override_shell, Some(false));
    }

    #[tokio::test]
    async fn test_build_agent_config_resolves_path_skills() {
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

        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config should resolve path skill");

        let prompt = config.system_prompt.expect("system prompt");
        assert!(prompt.contains("Path skill content for leader."));
        assert!(!prompt.contains("[skill from:"));
    }

    #[tokio::test]
    async fn test_build_agent_config_fails_for_missing_path_skill() {
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

        let err = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect_err("missing path skill should fail");

        match err {
            MobError::Internal(message) => {
                assert!(message.contains("failed to read skill file"));
                assert!(message.contains(&missing_path));
            }
            other => panic!("expected MobError::Internal, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_build_agent_config_passes_app_context() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let ctx = serde_json::json!({"key": "val"});
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: Some(ctx.clone()),
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.app_context,
            Some(ctx),
            "app_context should be passed through to AgentBuildConfig"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_none_context() {
        let def = sample_definition();
        let lead = &def.profiles[&ProfileName::from("lead")];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            meerkat_id: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.app_context, None,
            "app_context should be None when no context is provided"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_app_labels_overwritten_by_mob_labels() {
        let def = sample_definition();
        let worker = &def.profiles[&ProfileName::from("worker")];
        let mut app_labels = std::collections::BTreeMap::new();
        app_labels.insert("faction".to_string(), "north".to_string());
        // Attempt to override mob_id should be overwritten
        app_labels.insert("mob_id".to_string(), "sneaky-override".to_string());

        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            meerkat_id: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: Some(app_labels),
        })
        .await
        .expect("build_agent_config");

        let meta = config.peer_meta.as_ref().expect("peer_meta should be set");
        // App label should be present
        assert_eq!(
            meta.labels.get("faction").map(String::as_str),
            Some("north"),
            "app labels should be present in peer_meta"
        );
        // Mob standard labels should overwrite the sneaky override
        assert_eq!(
            meta.labels.get("mob_id").map(String::as_str),
            Some("test-mob"),
            "mob standard labels must overwrite app labels on conflict"
        );
        assert_eq!(meta.labels.get("role").map(String::as_str), Some("worker"));
        assert_eq!(
            meta.labels.get("meerkat_id").map(String::as_str),
            Some("w-1")
        );
    }
}
