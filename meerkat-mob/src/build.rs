//! Profile to AgentBuildConfig compilation.
//!
//! Maps a mob [`Profile`] to an [`AgentBuildConfig`] with the correct
//! flags for keep-alive operation, comms naming, peer metadata, and
//! tool overrides. Bridges to [`CreateSessionRequest`] for session creation.

use crate::definition::{MobDefinition, SkillSource};
use crate::error::MobError;
use crate::ids::{MeerkatId, MobId, ProfileName};
use crate::profile::Profile;
use meerkat::AgentBuildConfig;
use meerkat_core::PeerMeta;
use meerkat_core::RealmId;
use meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY;
use meerkat_core::Session;
use meerkat_core::SessionToolVisibilityState;
use meerkat_core::ToolCategoryOverride;
use meerkat_core::WitnessedToolFilter;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, MobToolAuthorityContext,
    resolve_mob_operator_access,
};
use meerkat_core::session::SessionMetadata;
use meerkat_core::types::SessionId;
use std::sync::Arc;

fn mob_realm_id(mob_id: &MobId) -> Result<RealmId, MobError> {
    RealmId::parse(format!("mob.{mob_id}")).map_err(|e| {
        MobError::WiringError(format!(
            "mob id '{mob_id}' cannot be used as a realm identity: {e}"
        ))
    })
}

/// Derive the effective `(override_mob, authority)` for a profile.
///
/// `profile.tools.mob` is the policy declaration.
/// The canonical resolver `resolve_mob_operator_access` synthesizes a typed
/// `MobToolAuthorityContext` (defaulting to a generated create-only shape) when
/// the profile says enable and no persisted authority is supplied. This is the
/// single source of truth for both build-time `override_mob` and runtime tool
/// dispatcher mounting; do not invent a parallel rule.
pub(crate) fn resolve_profile_mob_operator_access(
    profile: &Profile,
    persisted_authority: Option<MobToolAuthorityContext>,
) -> (ToolCategoryOverride, Option<MobToolAuthorityContext>) {
    let enable_mob = ToolCategoryOverride::from_effective(profile.tools.mob);
    resolve_mob_operator_access(enable_mob, persisted_authority)
}

/// Open profile tool categories for an already-witnessed inherited filter.
///
/// `SpawnTooling::InheritParent` and `SpawnTooling::Minimal` derive the actual
/// child-visible tool set from the parent's ToolScope snapshot. In that mode,
/// the selected mob profile still contributes model/skills/runtime metadata,
/// but its category booleans must not pre-disable tools before the inherited
/// allow-list is applied.
pub(crate) fn open_profile_tool_categories_for_inherited_filter(profile: &mut Profile) {
    profile.tools.builtins = true;
    profile.tools.shell = true;
    profile.tools.comms = true;
    profile.tools.memory = true;
    profile.tools.mob = true;
    profile.tools.schedule = true;
    profile.tools.image_generation = true;
    profile.tools.mcp.clear();
}

/// Parameters for building an agent config from a mob profile.
pub struct BuildAgentConfigParams<'a> {
    pub mob_id: &'a MobId,
    pub profile_name: &'a ProfileName,
    pub(crate) agent_identity: &'a MeerkatId,
    pub profile: &'a Profile,
    pub definition: &'a MobDefinition,
    pub external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    pub context: Option<serde_json::Value>,
    pub labels: Option<std::collections::BTreeMap<String, String>>,
    pub additional_instructions: Option<Vec<String>>,
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Persisted mob operator authority context (rehydration only).
    ///
    /// `None` means "no persisted authority" — when the profile says enable,
    /// the canonical resolver synthesizes a generated `create_only` shape.
    /// `Some(authority)` carries forward an already-issued capability scope
    /// (typically restored from event-sourced session metadata) and the
    /// resolver preserves it.
    pub mob_tool_authority_context: Option<MobToolAuthorityContext>,
    /// Pre-resolved inherited tool filter from spawn tooling.
    ///
    /// When set, stored in canonical session tool-visibility state so the
    /// runtime-backed core build restores it through the machine owner.
    pub inherited_tool_filter: Option<WitnessedToolFilter>,
}

pub struct BuildResumedAgentConfigParams<'a> {
    pub base: BuildAgentConfigParams<'a>,
    pub(crate) expected_session_id: &'a SessionId,
    pub resumed_session: Session,
}

/// Build an [`AgentBuildConfig`] from a mob profile.
///
/// This is the first step in the construction chain:
///   Profile -> `build_agent_config()` -> `to_create_session_request()` -> `SessionService::create_session()`
///
/// Mob-managed sessions are created with `keep_alive=false` by default.
/// Callers override `keep_alive` to `true` for `AutonomousHost` members
/// so the agent loop blocks until interrupted. Lifecycles are managed
/// explicitly by mob runtime commands (`start_turn`, `retire`, etc.).
pub async fn build_agent_config(
    params: BuildAgentConfigParams<'_>,
) -> Result<AgentBuildConfig, MobError> {
    let BuildAgentConfigParams {
        mob_id,
        profile_name,
        agent_identity,
        profile,
        definition,
        external_tools,
        context,
        labels,
        additional_instructions,
        shell_env,
        mob_tool_authority_context,
        inherited_tool_filter,
    } = params;

    if !profile.tools.comms {
        return Err(MobError::WiringError(format!(
            "profile '{profile_name}' has tools.comms=false; mob meerkats require comms=true"
        )));
    }

    // Comms name: "{mob_id}/{profile}/{meerkat_id}"
    let comms_name = format!("{mob_id}/{profile_name}/{agent_identity}");

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
        .with_label("meerkat_id", agent_identity.as_str());

    let realm_id = mob_realm_id(mob_id)?;

    // Assemble system prompt from profile skills (inline/path-based).
    let system_prompt = assemble_system_prompt(profile, definition).await?;

    let mut config = AgentBuildConfig::new(profile.model.clone());
    config.keep_alive = false;
    config.comms_name = Some(comms_name);
    config.peer_meta = Some(peer_meta);
    config.realm_id = Some(realm_id.to_string());
    if !system_prompt.is_empty() {
        config.system_prompt = Some(system_prompt);
    }

    // Mob comms instructions are delivered as an embedded skill via
    // preload_skills. The `skills` feature is required on the meerkat
    // dependency (enforced in Cargo.toml) so the skill engine is
    // always available. Skills are appended as extra_sections in
    // prompt assembly, which survives per-request system_prompt
    // overrides.
    config.preload_skills = Some(vec![meerkat_core::skills::SkillKey::builtin(
        meerkat_core::skills::SkillName::parse("mob-communication")
            .expect("mob-communication is a valid builtin skill slug"),
    )]);

    // Mob lifecycle notifications are typed at peer ingress. Do not rely on
    // silent_comms_intents string matching for canonical routing.
    config.silent_comms_intents = Vec::new();
    config.max_inline_peer_notifications = profile.max_inline_peer_notifications;

    // Map ToolConfig booleans to typed override intent.
    config.override_builtins =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.builtins);
    config.override_shell = meerkat_core::ToolCategoryOverride::from_effective(profile.tools.shell);
    config.override_memory =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.memory);
    config.override_schedule =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.schedule);
    config.override_image_generation =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.image_generation);
    let (override_mob, authority) =
        resolve_profile_mob_operator_access(profile, mob_tool_authority_context);
    config.override_mob = override_mob;
    config.mob_tool_authority_context = authority;

    // External tools (mob tools, task tools, rust bundles composed externally)
    config.external_tools = external_tools;

    // Opaque application context passed through to the agent build pipeline
    config.app_context = context;
    config.additional_instructions = additional_instructions;
    config.shell_env = shell_env;
    config.provider_params = profile.provider_params.clone();

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

    // Inherited tool filter: inject canonical visibility metadata so the
    // factory-backed core build restores it through the runtime owner.
    if let Some(authority) = inherited_tool_filter {
        meerkat_core::tool_scope::validate_witnessed_filter_authority(
            &authority.filter,
            &authority.witnesses,
        )
        .map_err(|err| {
            MobError::WiringError(format!("invalid inherited tool visibility: {err}"))
        })?;
        if let Ok(value) = serde_json::to_value(SessionToolVisibilityState {
            inherited_base_filter: authority.filter,
            filter_witnesses: authority.witnesses,
            ..Default::default()
        }) {
            config
                .initial_metadata_entries
                .insert(SESSION_TOOL_VISIBILITY_STATE_KEY.to_string(), value);
        }
    }

    Ok(config)
}

/// Build an [`AgentBuildConfig`] for a resumed mob member.
///
/// This preserves durable session identity from the stored session while still
/// composing current runtime mechanics such as external tool dispatchers and
/// realm attachment.
pub async fn build_resumed_agent_config(
    params: BuildResumedAgentConfigParams<'_>,
) -> Result<AgentBuildConfig, MobError> {
    let BuildResumedAgentConfigParams {
        base,
        expected_session_id,
        mut resumed_session,
    } = params;
    let inherited_tool_filter = base.inherited_tool_filter.clone();
    if resumed_session.id() != expected_session_id {
        return Err(MobError::Internal(format!(
            "resume session id mismatch: expected '{}', got '{}'",
            expected_session_id,
            resumed_session.id()
        )));
    }
    let mut config = build_agent_config(base).await?;
    config
        .initial_metadata_entries
        .remove(SESSION_TOOL_VISIBILITY_STATE_KEY);
    merge_inherited_filter_into_resumed_visibility(&mut resumed_session, inherited_tool_filter)?;
    let metadata = resumed_session
        .session_metadata()
        .ok_or_else(|| MobError::Internal("missing durable session metadata".to_string()))?;
    apply_resumed_session_metadata(&mut config, &metadata)?;
    config.resume_session = Some(resumed_session);
    // Preserve the durable session prompt/history exactly as stored.
    config.system_prompt = None;
    // Do not silently reapply prompt-affecting surface-local context on resume.
    config.additional_instructions = None;
    config.app_context = None;
    config.shell_env = None;
    Ok(config)
}

fn merge_inherited_filter_into_resumed_visibility(
    session: &mut Session,
    inherited_tool_filter: Option<WitnessedToolFilter>,
) -> Result<(), MobError> {
    let Some(authority) = inherited_tool_filter else {
        return Ok(());
    };
    meerkat_core::tool_scope::validate_witnessed_filter_authority(
        &authority.filter,
        &authority.witnesses,
    )
    .map_err(|err| MobError::Internal(format!("invalid inherited tool visibility: {err}")))?;
    let mut visibility_state = session
        .try_tool_visibility_state()
        .map_err(|err| {
            MobError::Internal(format!(
                "invalid canonical tool visibility state for resumed mob member: {err}"
            ))
        })?
        .unwrap_or_default();
    visibility_state.inherited_base_filter = authority.filter;
    visibility_state
        .filter_witnesses
        .extend(authority.witnesses);
    session
        .set_tool_visibility_state(visibility_state)
        .map_err(|err| {
            MobError::Internal(format!(
                "failed to merge inherited tool visibility into resumed mob member: {err}"
            ))
        })
}

fn apply_resumed_session_metadata(
    config: &mut AgentBuildConfig,
    metadata: &SessionMetadata,
) -> Result<(), MobError> {
    let current_comms_name = config.comms_name.clone();
    let Some(stored_comms_name) = metadata.comms_name.clone() else {
        return Err(MobError::Internal(
            "missing durable comms_name for resumed mob member".to_string(),
        ));
    };
    if current_comms_name.as_deref() != Some(stored_comms_name.as_str()) {
        return Err(MobError::Internal(format!(
            "persisted comms_name '{}' does not match current mob identity '{}'",
            stored_comms_name,
            current_comms_name.unwrap_or_else(|| "<none>".to_string())
        )));
    }

    config.model = metadata.model.clone();
    config.max_tokens = Some(metadata.max_tokens);
    config.provider = Some(metadata.provider);
    config.provider_params = metadata.provider_params.clone();
    config.override_builtins = metadata.tooling.builtins;
    config.override_shell = metadata.tooling.shell;
    config.override_memory = metadata.tooling.memory;
    config.override_image_generation = metadata.tooling.image_generation;
    if matches!(
        config.override_mob,
        meerkat_core::ToolCategoryOverride::Inherit
    ) {
        config.override_mob = metadata.tooling.mob;
    }
    config.preload_skills = metadata.tooling.active_skills.clone();
    // keep_alive is NOT restored from metadata — mob runtime owns it
    // (determined by runtime_mode == AutonomousHost). §1: one owner.
    config.comms_name = Some(stored_comms_name);
    config.peer_meta = metadata.peer_meta.clone();
    Ok(())
}

/// Bridge an [`AgentBuildConfig`] to a [`CreateSessionRequest`].
///
/// This is the second step: the config is converted to the service-level
/// request type that `SessionService::create_session()` accepts.
pub fn to_create_session_request(
    config: &AgentBuildConfig,
    prompt: meerkat_core::types::ContentInput,
) -> CreateSessionRequest {
    let build_options = config.to_session_build_options();

    CreateSessionRequest {
        model: config.model.clone(),
        prompt,
        render_metadata: None,
        system_prompt: config.system_prompt.clone(),
        max_tokens: config.max_tokens,
        event_tx: None,

        skill_references: None,
        // Mob runtime owns lifecycle startup and starts autonomous host loops
        // explicitly after provisioning. Avoid synchronous first-turn execution
        // during create_session so spawn does not block on LLM latency, and do
        // not stage the kickoff prompt here because the runtime will send it
        // explicitly on the first real turn.
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
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
    use crate::profile::{ProfileBinding, ToolConfig};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            ProfileBinding::Inline(Profile {
                model: "claude-opus-4-6".into(),
                skills: vec!["leader-skill".into()],
                tools: ToolConfig {
                    builtins: true,
                    shell: true,
                    comms: true,
                    memory: false,
                    mob: true,
                    schedule: false,
                    image_generation: true,
                    mcp: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "Orchestrates the mob".into(),
                external_addressable: true,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Profile {
                model: "claude-sonnet-4-5".into(),
                skills: vec![],
                tools: ToolConfig {
                    builtins: true,
                    shell: false,
                    comms: true,
                    memory: false,
                    mob: false,
                    schedule: false,
                    image_generation: false,
                    mcp: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "Does work".into(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
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
            wiring: WiringRules::default(),
            skills,
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_bridge_session_id: None,
            session_cleanup_policy: crate::definition::SessionCleanupPolicy::Manual,
            is_implicit: false,
        }
    }

    fn injected_authority() -> Option<MobToolAuthorityContext> {
        Some(
            meerkat_core::service::MobToolAuthorityContext::new(
                meerkat_core::service::OpaquePrincipalToken::new("test-principal"),
                true,
            )
            .with_managed_mob_scope(["test-mob"]),
        )
    }

    fn witnessed_filter(
        filter: meerkat_core::tool_scope::ToolFilter,
        names: &[&str],
    ) -> WitnessedToolFilter {
        WitnessedToolFilter::new(
            filter,
            names
                .iter()
                .map(|name| {
                    (
                        (*name).to_string(),
                        meerkat_core::ToolVisibilityWitness {
                            stable_owner_key: Some(format!("test-owner:{name}")),
                            last_seen_provenance: None,
                        },
                    )
                })
                .collect(),
        )
    }

    fn resumed_session_with_metadata(session_id: SessionId) -> Session {
        let mut resumed_session = Session::with_id(session_id);
        resumed_session
            .set_session_metadata(SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-opus-4-6".to_string(),
                max_tokens: 2048,
                structured_output_retries: 2,
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::session::SessionTooling {
                    builtins: meerkat_core::session::ToolCategoryOverride::Enable,
                    shell: meerkat_core::session::ToolCategoryOverride::Enable,
                    comms: meerkat_core::session::ToolCategoryOverride::Enable,
                    mob: meerkat_core::session::ToolCategoryOverride::Enable,
                    memory: meerkat_core::session::ToolCategoryOverride::Disable,
                    workgraph: meerkat_core::session::ToolCategoryOverride::Inherit,
                    image_generation: meerkat_core::session::ToolCategoryOverride::Enable,
                    web_search: meerkat_core::session::ToolCategoryOverride::Inherit,
                    active_skills: None,
                },
                keep_alive: false,
                comms_name: Some("test-mob/lead/lead-1".to_string()),
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
            })
            .expect("session metadata");
        resumed_session
    }

    #[tokio::test]
    async fn test_build_agent_config_non_keep_alive() {
        let def = sample_definition();
        let profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert!(!config.keep_alive, "keep_alive must be false for mob spawn");
    }

    #[tokio::test]
    async fn test_build_agent_config_comms_name() {
        let def = sample_definition();
        let profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        let profile = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &MeerkatId::from("w-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        let profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.realm_id.as_deref(),
            Some("mob.test-mob"),
            "realm_id should be a canonical mob realm slug"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_tool_overrides() {
        let def = sample_definition();

        // Lead profile has builtins=true, shell=true, memory=false
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");
        assert_eq!(
            config.override_builtins,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_shell,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_memory,
            meerkat_core::ToolCategoryOverride::Disable
        );
        assert_eq!(
            config.override_image_generation,
            meerkat_core::ToolCategoryOverride::Enable
        );
        // Lead profile declares tools.mob = true; with no persisted authority
        // the canonical resolver synthesizes a generated create-only shape and
        // override_mob is Enable (not Disable as in the pre-canonicalization
        // shadow path).
        assert_eq!(
            config.override_mob,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert!(config.mob_tool_authority_context.is_some());
        // Worker profile has builtins=true, shell=false, memory=false
        let worker = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");
        assert_eq!(
            config.override_builtins,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_shell,
            meerkat_core::ToolCategoryOverride::Disable
        );
        assert_eq!(
            config.override_memory,
            meerkat_core::ToolCategoryOverride::Disable
        );
        assert_eq!(
            config.override_image_generation,
            meerkat_core::ToolCategoryOverride::Disable
        );
        assert_eq!(
            config.override_mob,
            meerkat_core::ToolCategoryOverride::Disable
        );
    }

    #[tokio::test]
    async fn test_inherited_tooling_opens_profile_category_caps() {
        let def = sample_definition();
        let mut profile = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap()
            .clone();
        profile.tools.mcp = vec!["narrow-mcp-source".to_string()];

        open_profile_tool_categories_for_inherited_filter(&mut profile);
        assert_eq!(
            profile.tools.mcp,
            Vec::<String>::new(),
            "inherited tooling should not keep profile-level MCP source caps"
        );

        let inherited_filter = meerkat_core::tool_scope::ToolFilter::Allow(
            ["bash".to_string(), "mob_spawn_member".to_string()]
                .into_iter()
                .collect(),
        );
        let inherited_authority =
            witnessed_filter(inherited_filter.clone(), &["bash", "mob_spawn_member"]);
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &MeerkatId::from("w-inherit"),
            profile: &profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: Some(inherited_authority),
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.override_builtins,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_shell,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_memory,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_image_generation,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            config.override_mob,
            meerkat_core::ToolCategoryOverride::Enable
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_routes_inherited_filter_through_canonical_visibility_state() {
        let def = sample_definition();
        let profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let inherited_filter =
            meerkat_core::tool_scope::ToolFilter::Deny(["shell".to_string()].into_iter().collect());
        let inherited_authority = witnessed_filter(inherited_filter.clone(), &["shell"]);
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: Some(inherited_authority.clone()),
        })
        .await
        .expect("build_agent_config");

        assert!(
            !config
                .initial_metadata_entries
                .contains_key(meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY),
            "mob build must not write legacy inherited visibility metadata"
        );
        let visibility_state = config
            .initial_metadata_entries
            .get(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY)
            .and_then(|value| {
                serde_json::from_value::<meerkat_core::SessionToolVisibilityState>(value.clone())
                    .ok()
            })
            .expect("canonical visibility metadata should be present");
        assert_eq!(
            visibility_state.inherited_base_filter, inherited_filter,
            "inherited mob filter should flow through canonical visibility state"
        );
        assert_eq!(
            visibility_state.filter_witnesses, inherited_authority.witnesses,
            "inherited mob filter witnesses should flow through canonical visibility state"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_rejects_name_only_inherited_filter() {
        let def = sample_definition();
        let profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let err = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: Some(WitnessedToolFilter::new(
                meerkat_core::tool_scope::ToolFilter::Allow(
                    ["shell".to_string()].into_iter().collect(),
                ),
                Default::default(),
            )),
        })
        .await
        .expect_err("name-only inherited filter should fail closed");

        assert!(
            err.to_string().contains("shell"),
            "rejection should name the missing inherited filter witness: {err}"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_operator_context_enables_mob_override() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-operator"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: injected_authority(),
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.override_mob,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert!(
            config.mob_tool_authority_context.is_some(),
            "typed injected authority should flow into the build config"
        );
    }

    #[tokio::test]
    async fn test_build_resumed_agent_config_uses_profile_intent_for_mob_override() {
        // The profile is the canonical source of mob override intent. On resume
        // with no persisted authority injected, the resolver synthesizes a
        // generated create-only authority — equivalent to a fresh build of the
        // same profile. Old metadata cannot quietly demote the profile's
        // declared intent.
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        assert!(
            lead.tools.mob,
            "fixture must declare profile.tools.mob = true"
        );
        let session_id = SessionId::new();
        let resumed_session = resumed_session_with_metadata(session_id.clone());

        let config = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &ProfileName::from("lead"),
                agent_identity: &MeerkatId::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                inherited_tool_filter: None,
            },
            expected_session_id: &session_id,
            resumed_session,
        })
        .await
        .expect("build_resumed_agent_config");

        assert_eq!(
            config.override_mob,
            meerkat_core::ToolCategoryOverride::Enable,
            "profile.tools.mob = true must yield Enable on resume; the canonical resolver \
             synthesizes a generated create-only authority when none is persisted"
        );
        assert!(
            config.mob_tool_authority_context.is_some(),
            "resolver must synthesize an authority context when profile says enable"
        );
    }

    #[tokio::test]
    async fn test_build_resumed_agent_config_merges_inherited_filter_into_existing_visibility_state()
     {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let session_id = SessionId::new();
        let mut resumed_session = resumed_session_with_metadata(session_id.clone());
        let inherited_filter = meerkat_core::tool_scope::ToolFilter::Deny(
            ["parent_shell".to_string()].into_iter().collect(),
        );
        let inherited_authority = witnessed_filter(inherited_filter.clone(), &["parent_shell"]);
        let original_state = SessionToolVisibilityState {
            inherited_base_filter: meerkat_core::tool_scope::ToolFilter::Deny(
                ["old_parent".to_string()].into_iter().collect(),
            ),
            active_filter: meerkat_core::tool_scope::ToolFilter::Deny(
                ["active_secret".to_string()].into_iter().collect(),
            ),
            staged_filter: meerkat_core::tool_scope::ToolFilter::Allow(
                ["staged_visible".to_string()].into_iter().collect(),
            ),
            active_requested_deferred_names: BTreeSet::from(["deferred_active".to_string()]),
            staged_requested_deferred_names: BTreeSet::from(["deferred_staged".to_string()]),
            active_revision: 7,
            staged_revision: 9,
            requested_witnesses: [(
                "deferred_active".to_string(),
                meerkat_core::ToolVisibilityWitness {
                    stable_owner_key: Some("owner:deferred_active".to_string()),
                    last_seen_provenance: None,
                },
            )]
            .into_iter()
            .collect(),
            filter_witnesses: [(
                "active_secret".to_string(),
                meerkat_core::ToolVisibilityWitness {
                    stable_owner_key: Some("owner:active_secret".to_string()),
                    last_seen_provenance: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        resumed_session
            .set_tool_visibility_state(original_state.clone())
            .expect("visibility state");

        let config = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &ProfileName::from("lead"),
                agent_identity: &MeerkatId::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                inherited_tool_filter: Some(inherited_authority.clone()),
            },
            expected_session_id: &session_id,
            resumed_session,
        })
        .await
        .expect("build_resumed_agent_config");

        assert!(
            !config
                .initial_metadata_entries
                .contains_key(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            "resumed mob config must not stage replacement canonical visibility metadata"
        );
        let visibility_state = config
            .resume_session
            .as_ref()
            .expect("resume session")
            .try_tool_visibility_state()
            .expect("parse visibility")
            .expect("visibility state");
        assert_eq!(visibility_state.inherited_base_filter, inherited_filter);
        assert_eq!(visibility_state.active_filter, original_state.active_filter);
        assert_eq!(visibility_state.staged_filter, original_state.staged_filter);
        assert_eq!(
            visibility_state.active_requested_deferred_names,
            original_state.active_requested_deferred_names
        );
        assert_eq!(
            visibility_state.staged_requested_deferred_names,
            original_state.staged_requested_deferred_names
        );
        assert_eq!(
            visibility_state.active_revision,
            original_state.active_revision
        );
        assert_eq!(
            visibility_state.staged_revision,
            original_state.staged_revision
        );
        assert_eq!(
            visibility_state.requested_witnesses,
            original_state.requested_witnesses
        );
        let mut expected_filter_witnesses = original_state.filter_witnesses.clone();
        expected_filter_witnesses.extend(inherited_authority.witnesses);
        assert_eq!(visibility_state.filter_witnesses, expected_filter_witnesses);
    }

    #[tokio::test]
    async fn test_build_resumed_agent_config_rejects_malformed_visibility_state_before_merge() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let session_id = SessionId::new();
        let mut resumed_session = resumed_session_with_metadata(session_id.clone());
        resumed_session.set_metadata(
            meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY,
            serde_json::json!("not-a-visibility-state"),
        );

        let err = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &ProfileName::from("lead"),
                agent_identity: &MeerkatId::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                inherited_tool_filter: Some(witnessed_filter(
                    meerkat_core::tool_scope::ToolFilter::Deny(
                        ["parent_shell".to_string()].into_iter().collect(),
                    ),
                    &["parent_shell"],
                )),
            },
            expected_session_id: &session_id,
            resumed_session,
        })
        .await
        .expect_err("malformed canonical visibility must fail closed");

        assert!(
            err.to_string()
                .contains("invalid canonical tool visibility state"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_fails_when_comms_disabled() {
        let mut def = sample_definition();
        def.profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile")
            .as_inline_mut()
            .unwrap()
            .tools
            .comms = false;
        let worker = def
            .profiles
            .get(&ProfileName::from("worker"))
            .expect("worker profile")
            .as_inline()
            .unwrap();

        let result = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        let preload = config
            .preload_skills
            .as_ref()
            .expect("preload_skills should be set");
        assert!(
            preload
                .iter()
                .any(|id| id.skill_name.as_str() == "mob-communication"),
            "preload_skills should include mob-communication"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_does_not_rely_on_silent_comms_intents_for_mob_lifecycle() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert!(
            config.silent_comms_intents.is_empty(),
            "mob lifecycle routing should no longer depend on silent_comms_intents"
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
            .as_inline_mut()
            .unwrap()
            .max_inline_peer_notifications = Some(15);
        let lead = def.profiles[&lead_key].as_inline().unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &lead_key,
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.max_inline_peer_notifications, Some(15));
    }

    #[tokio::test]
    async fn test_build_agent_config_model() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.model, "claude-opus-4-6");
    }

    #[tokio::test]
    async fn test_to_create_session_request() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Hello mob".to_string().into());
        assert_eq!(req.model, "claude-opus-4-6");
        assert_eq!(req.prompt.text_content(), "Hello mob");
        assert!(req.system_prompt.is_some());
        assert_eq!(
            req.initial_turn,
            meerkat_core::service::InitialTurnPolicy::Defer
        );
        assert_eq!(req.deferred_prompt_policy, DeferredPromptPolicy::Discard);

        let build = req.build.expect("build options should be set");
        assert_eq!(build.comms_name.as_deref(), Some("test-mob/lead/lead-1"));
        assert!(build.peer_meta.is_some());
        assert_eq!(build.realm_id.as_deref(), Some("mob.test-mob"));
        assert_eq!(
            build.override_builtins,
            meerkat_core::ToolCategoryOverride::Enable
        );
        assert_eq!(
            build.override_shell,
            meerkat_core::ToolCategoryOverride::Enable
        );
    }

    #[tokio::test]
    async fn test_to_create_session_request_worker() {
        let def = sample_definition();
        let worker = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Start working".to_string().into());
        assert_eq!(req.model, "claude-sonnet-4-5");
        assert_eq!(req.deferred_prompt_policy, DeferredPromptPolicy::Discard);
        let build = req.build.expect("build options");
        assert_eq!(
            build.override_shell,
            meerkat_core::ToolCategoryOverride::Disable
        );
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
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("lead profile")
            .as_inline_mut()
            .unwrap()
            .skills
            .push("path-skill".into());
        let lead = def
            .profiles
            .get(&ProfileName::from("lead"))
            .expect("lead profile")
            .as_inline()
            .unwrap();

        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        def.profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("lead profile")
            .as_inline_mut()
            .unwrap()
            .skills = vec!["missing-path-skill".into()];
        let lead = def
            .profiles
            .get(&ProfileName::from("lead"))
            .expect("lead profile")
            .as_inline()
            .unwrap();

        let err = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let ctx = serde_json::json!({"key": "val"});
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: Some(ctx.clone()),
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.app_context, None,
            "app_context should be None when no context is provided"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_passes_provider_params() {
        let mut def = sample_definition();
        let lead_key = ProfileName::from("lead");
        def.profiles
            .get_mut(&lead_key)
            .expect("lead profile")
            .as_inline_mut()
            .unwrap()
            .provider_params = Some(serde_json::json!({
            "thinking_budget": 4096,
            "top_k": 40
        }));

        let lead = def.profiles[&lead_key].as_inline().unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &lead_key,
            agent_identity: &MeerkatId::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.provider_params,
            Some(serde_json::json!({
                "thinking_budget": 4096,
                "top_k": 40
            })),
            "provider_params should be passed through to AgentBuildConfig"
        );

        let req = to_create_session_request(&config, "hello".to_string().into());
        let build = req.build.expect("build options should be set");
        assert_eq!(
            build.provider_params,
            Some(serde_json::json!({
                "thinking_budget": 4096,
                "top_k": 40
            })),
            "provider_params should survive into SessionBuildOptions"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_app_labels_overwritten_by_mob_labels() {
        let def = sample_definition();
        let worker = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap();
        let mut app_labels = std::collections::BTreeMap::new();
        app_labels.insert("faction".to_string(), "north".to_string());
        // Attempt to override mob_id should be overwritten
        app_labels.insert("mob_id".to_string(), "sneaky-override".to_string());

        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &MeerkatId::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: Some(app_labels),
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            inherited_tool_filter: None,
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
