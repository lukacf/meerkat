//! Profile to AgentBuildConfig compilation.
//!
//! Maps a mob [`Profile`] to an [`AgentBuildConfig`] with the correct
//! flags for keep-alive operation, comms naming, peer metadata, and
//! tool overrides. Bridges to [`CreateSessionRequest`] for session creation.

use crate::definition::{MobDefinition, SkillSource};
use crate::error::MobError;
use crate::ids::{AgentIdentity, MobId, ProfileName};
use crate::profile::Profile;
use meerkat::AgentBuildConfig;
use meerkat::SystemPromptOverride;
use meerkat_core::InheritedToolVisibilityAuthority;
use meerkat_core::PeerMeta;
use meerkat_core::RealmId;
use meerkat_core::Session;
use meerkat_core::SessionLlmIdentity;
use meerkat_core::SessionToolVisibilityState;
use meerkat_core::ToolCategoryOverride;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, MobToolAuthorityContext, ResumeOverrideMask,
};
use meerkat_core::session::SessionMetadata;
use meerkat_core::types::SessionId;
use std::sync::Arc;

fn mob_realm_id(mob_id: &MobId) -> Result<RealmId, MobError> {
    // Shared dot-form realm helper (meerkat-core) — the single owner of the
    // `mob.{id}` realm form. Producer (here) and consumer (mob-mcp ownership
    // routing) derive the realm through the same helper, so the dot/colon
    // divergence that previously broke `persisted_mob_binding` cannot recur.
    meerkat_core::mob_realm_id(mob_id.as_str()).map_err(|e| {
        MobError::WiringError(format!(
            "mob id '{mob_id}' cannot be used as a realm identity: {e}"
        ))
    })
}

fn builtin_skill_key(name: &str) -> meerkat_core::skills::SkillKey {
    meerkat_core::skills::SkillKey::builtin(
        meerkat_core::skills::SkillName::parse(name)
            .expect("mob build preloads only valid builtin skill slugs"),
    )
}

/// Derive the effective `(override_mob, authority)` for a profile.
///
/// `profile.tools.mob` is the policy declaration.
/// The generated MeerkatMachine resolver synthesizes a typed
/// `MobToolAuthorityContext` when the profile says enable and no persisted
/// authority is supplied. This keeps build-time `override_mob` and dispatcher
/// mounting under the same generated authority path.
pub(crate) fn resolve_profile_mob_operator_access(
    profile: &Profile,
    persisted_authority: Option<MobToolAuthorityContext>,
) -> (ToolCategoryOverride, Option<MobToolAuthorityContext>) {
    let enable_mob = ToolCategoryOverride::from_effective(profile.tools.mob);
    meerkat_runtime::mob_operator_authority::resolve_mob_operator_access(
        enable_mob,
        persisted_authority,
    )
}

fn stamp_standard_mob_member_labels(
    peer_meta: PeerMeta,
    binding: &meerkat_core::MobMemberBinding,
) -> PeerMeta {
    peer_meta
        .with_label("mob_id", binding.mob_id.as_str())
        .with_label("role", binding.role.as_str())
        .with_label("meerkat_id", binding.member.as_str())
        .with_label("agent_identity", binding.member.as_str())
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
    profile.tools.workgraph = true;
    profile.tools.mob = true;
    profile.tools.schedule = true;
    profile.tools.image_generation = true;
    profile.tools.mcp.clear();
}

/// Parameters for building an agent config from a mob profile.
pub struct BuildAgentConfigParams<'a> {
    pub mob_id: &'a MobId,
    pub profile_name: &'a ProfileName,
    pub(crate) agent_identity: &'a AgentIdentity,
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
    pub inherited_tool_filter: Option<InheritedToolVisibilityAuthority>,
    /// Per-launch call-level tool access policy from the spawn spec.
    ///
    /// `AllowList`/`DenyList` ride into `AgentBuildConfig.tool_access_policy`
    /// and become the factory's outermost dispatch gate. `Inherit` reaching
    /// this compiler is host/operator authority — agent-facing spawn surfaces
    /// resolve `Inherit` to the parent's persisted effective policy before
    /// the spec reaches the actor — and resolves to unrestricted.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// Typed per-spawn system prompt replacement.
    pub system_prompt_override: Option<crate::runtime::SpawnSystemPromptOverride>,
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
        tool_access_policy,
        system_prompt_override,
    } = params;

    if !profile.tools.comms {
        return Err(MobError::WiringError(format!(
            "profile '{profile_name}' has tools.comms=false; mob meerkats require comms=true"
        )));
    }

    // Typed durable member identity + transport routing name. The comms name is
    // rendered from the same typed binding via `MemberCommsName::Display`, so
    // the `mob_id/role/member` join has exactly one owner.
    let member_binding = meerkat_core::MobMemberBinding {
        mob_id: mob_id.as_str().to_string(),
        role: profile_name.as_str().to_string(),
        member: agent_identity.as_str().to_string(),
    };
    let comms_name = member_binding
        .comms_name()
        .map_err(|e| {
            MobError::WiringError(format!(
                "mob member identity ({mob_id}/{profile_name}/{agent_identity}) is not a valid comms name: {e}"
            ))
        })?
        .to_string();

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
    peer_meta = stamp_standard_mob_member_labels(peer_meta, &member_binding);

    let realm_id = mob_realm_id(mob_id)?;

    // Assemble system prompt from profile skills (inline/path-based).
    let system_prompt = assemble_system_prompt(profile, definition).await?;

    let mut config = AgentBuildConfig::new(profile.model.clone());
    config.keep_alive = false;
    config.comms_name = Some(comms_name);
    config.peer_meta = Some(peer_meta);
    config.realm_id = Some(realm_id);
    config.mob_member_binding = Some(member_binding);
    match system_prompt_override {
        Some(crate::runtime::SpawnSystemPromptOverride::Replace(prompt)) => {
            config.system_prompt = SystemPromptOverride::Set(prompt);
        }
        Some(crate::runtime::SpawnSystemPromptOverride::Disable) => {
            config.system_prompt = SystemPromptOverride::Disable;
        }
        None if !system_prompt.is_empty() => {
            config.system_prompt = SystemPromptOverride::Set(system_prompt);
        }
        None => {}
    }

    // Mob comms and task/work coordination instructions are delivered as
    // embedded skills via preload_skills. The `skills` feature is required on
    // the meerkat dependency (enforced in Cargo.toml) so the skill engine is
    // always available. Skills are appended as extra_sections in prompt
    // assembly, which survives per-request system_prompt overrides.
    let mut preload_skills = vec![builtin_skill_key("mob-communication")];
    if profile.tools.workgraph {
        preload_skills.push(builtin_skill_key("workgraph-workflow"));
    } else if profile.tools.builtins {
        preload_skills.push(builtin_skill_key("task-workflow"));
    }
    config.preload_skills = Some(preload_skills);

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
    config.override_workgraph =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.workgraph);
    config.override_schedule =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.schedule);
    config.override_image_generation =
        meerkat_core::ToolCategoryOverride::from_effective(profile.tools.image_generation);
    let should_grant_default_spawn_profiles =
        profile.tools.mob && mob_tool_authority_context.is_none();
    let (override_mob, mut authority) =
        resolve_profile_mob_operator_access(profile, mob_tool_authority_context);
    if should_grant_default_spawn_profiles && let Some(authority_context) = authority.take() {
        let profile_names = definition
            .profiles
            .keys()
            .map(|profile| profile.as_str().to_string())
            .collect::<Vec<_>>();
        authority = Some(
            meerkat_runtime::mob_operator_authority::grant_spawn_profiles_in_mob(
                &authority_context,
                mob_id.as_str(),
                profile_names,
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "generated mob operator authority rejected default spawn profiles: {error}"
                ))
            })?,
        );
    }
    config.override_mob = override_mob;
    config.mob_tool_authority_context = authority;

    // External tools (mob tools, task tools, rust bundles composed externally)
    config.external_tools = external_tools;

    // Declarative MCP servers ride the durable profile, so revival
    // recomposes them without the lossy in-process spawn overlay.
    config.mcp_servers = profile.tools.mcp_servers.clone();

    // Opaque application context passed through to the agent build pipeline
    config.app_context = context;
    config.additional_instructions = additional_instructions;
    config.shell_env = shell_env;
    config.provider_params = profile.provider_params.clone();

    // Profile-declared provider identity: the typed provider (parsed
    // fail-closed at profile ingress) and the optional self-hosted server
    // binding flow straight into the build config; the registry rejects a
    // provider that contradicts a catalogued model's owner at build time.
    config.provider = profile.provider;
    config.self_hosted_server_id = profile.self_hosted_server_id.clone();

    // Mob-scoped `[models.<id>]` entries ride the build config into the
    // effective model registry (single owner for provider inference,
    // compaction scaling, capability gates, and timeouts).
    config.custom_models = definition.models.clone();

    // `Auto` image-generation target default: profile-level wins over the
    // mob-level default; absent both, the planner falls back to the
    // session's effective text provider.
    config.image_generation_provider = profile
        .image_generation_provider
        .or(definition.image_generation_provider);

    // Per-profile compaction threshold override (typed non-zero at ingress).
    config.auto_compact_threshold_override = profile.auto_compact_threshold;

    // Declared resume-override intent: list-ed profile fields win over
    // durable session metadata when this member resumes.
    config.resume_override_mask = profile.resume_override_mask();

    // Structured output: the profile carries the typed, already-validated
    // schema (validated once at profile ingress).
    if let Some(schema) = &profile.output_schema {
        config.output_schema = Some(meerkat_core::OutputSchema {
            schema: schema.clone(),
            name: Some(format!("{mob_id}_{profile_name}")),
            strict: true,
            compat: Default::default(),
            format: Default::default(),
        });
    }

    // Inherited tool filter: carry typed visibility state to the
    // factory-backed core build so it restores through the runtime owner.
    if let Some(authority) = inherited_tool_filter {
        config.initial_tool_visibility_state = Some(authority);
    }

    // Call-level tool access policy: `AllowList`/`DenyList` become the
    // factory's outermost dispatch gate. `Inherit` at this seam is
    // host/operator authority (agent-facing spawn surfaces resolve `Inherit`
    // to the parent's persisted effective policy before the spec reaches the
    // actor) and resolves to unrestricted — the factory fails closed on any
    // `Inherit` that would otherwise leak through.
    config.tool_access_policy = match tool_access_policy {
        Some(meerkat_core::ops::ToolAccessPolicy::Inherit) | None => None,
        explicit => explicit,
    };

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
        resumed_session,
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
    if inherited_tool_filter.is_some() {
        resumed_session.try_tool_visibility_state().map_err(|err| {
            MobError::Internal(format!(
                "invalid canonical tool visibility state for resumed mob member: {err}"
            ))
        })?;
    }
    let metadata = resumed_session
        .session_metadata()
        .ok_or_else(|| MobError::Internal("missing durable session metadata".to_string()))?;
    apply_resumed_session_metadata(&mut config, &metadata)?;
    config.resume_session = Some(resumed_session);
    // Preserve the durable session prompt/history exactly as stored.
    config.system_prompt = SystemPromptOverride::Inherit;
    // Do not silently reapply prompt-affecting surface-local context on resume.
    config.additional_instructions = None;
    config.app_context = None;
    config.shell_env = None;
    Ok(config)
}

fn apply_resumed_session_metadata(
    config: &mut AgentBuildConfig,
    metadata: &SessionMetadata,
) -> Result<(), MobError> {
    let Some(current_comms_name) = config.comms_name.clone() else {
        return Err(MobError::Internal(
            "missing current comms_name for resumed mob member".to_string(),
        ));
    };
    let Some(stored_comms_name) = metadata.comms_name.clone() else {
        return Err(MobError::Internal(
            "missing durable comms_name for resumed mob member".to_string(),
        ));
    };
    if !resumed_comms_name_matches_current_or_legacy(&current_comms_name, &stored_comms_name) {
        return Err(MobError::Internal(format!(
            "persisted comms_name '{stored_comms_name}' does not match current mob identity '{current_comms_name}'"
        )));
    }
    if let Some(current_binding) = config.mob_member_binding.as_ref() {
        let current_binding_comms = current_binding.comms_name().map_err(|error| {
            MobError::Internal(format!(
                "current mob member binding cannot render a comms_name: {error}"
            ))
        })?;
        if current_binding_comms.to_string() != current_comms_name {
            return Err(MobError::Internal(format!(
                "current mob member binding '{current_binding_comms}' does not match current comms_name '{current_comms_name}'"
            )));
        }
    }
    if let Some(stored_binding) = metadata.mob_member_binding.as_ref() {
        let Some(current_binding) = config.mob_member_binding.as_ref() else {
            return Err(MobError::Internal(
                "persisted mob member binding has no current binding to resume into".to_string(),
            ));
        };
        let stored_binding_comms = stored_binding.comms_name().map_err(|error| {
            MobError::Internal(format!(
                "persisted mob member binding cannot render a comms_name: {error}"
            ))
        })?;
        if !resumed_member_binding_matches_current_or_legacy(current_binding, stored_binding) {
            return Err(MobError::Internal(format!(
                "persisted mob member binding '{stored_binding_comms}' does not match current mob identity '{current_comms_name}'"
            )));
        }
    }

    // Durable metadata restores only the fields the profile did not claim via
    // `resume_overrides` (typed mask, set from the profile in
    // `build_agent_config`). A masked field keeps the freshly-built profile
    // value so a definition edit applies to resumed durable sessions.
    let effective_identity = effective_resumed_session_llm_identity(
        SessionLlmIdentity {
            model: config.model.clone(),
            provider: config.provider.unwrap_or(metadata.provider),
            self_hosted_server_id: config.self_hosted_server_id.clone(),
            provider_params: config.provider_params.clone(),
            auth_binding: config.auth_binding.clone(),
        },
        metadata,
        config.resume_override_mask,
    );
    config.model = effective_identity.model;
    config.provider = Some(effective_identity.provider);
    config.self_hosted_server_id = effective_identity.self_hosted_server_id;
    config.provider_params = effective_identity.provider_params;
    config.auth_binding = effective_identity.auth_binding;
    config.max_tokens = Some(metadata.max_tokens);
    config.override_builtins = metadata.tooling.builtins;
    config.override_shell = metadata.tooling.shell;
    config.override_memory = metadata.tooling.memory;
    config.override_schedule = metadata.tooling.schedule;
    config.override_workgraph = metadata.tooling.workgraph;
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
    config.comms_name = Some(current_comms_name);
    config.peer_meta = metadata.peer_meta.clone();
    if let Some(binding) = config.mob_member_binding.clone() {
        let peer_meta = config.peer_meta.take().unwrap_or_default();
        config.peer_meta = Some(stamp_standard_mob_member_labels(peer_meta, &binding));
    }
    Ok(())
}

/// Resolve the exact LLM identity a mob resume will hand to AgentFactory.
///
/// The portable profile is the candidate; durable metadata wins for every
/// unmasked field. An explicit current binding keeps precedence, while an
/// omitted binding restores the durable configured binding. Both the actual
/// resumed build and member-host Tier-2 preflight use this one pure seam.
pub(crate) fn effective_resumed_session_llm_identity(
    mut candidate: SessionLlmIdentity,
    metadata: &SessionMetadata,
    mask: ResumeOverrideMask,
) -> SessionLlmIdentity {
    if !mask.model {
        candidate.model = metadata.model.clone();
    }
    if !mask.provider {
        candidate.provider = metadata.provider;
        candidate.self_hosted_server_id = metadata.self_hosted_server_id.clone();
    }
    if !mask.provider_params {
        candidate.provider_params = metadata.provider_params.clone();
    }
    if !mask.auth_binding && candidate.auth_binding.is_none() {
        candidate.auth_binding = metadata
            .auth_binding
            .clone()
            .filter(|binding| !binding.is_env_default());
    }
    candidate
}

pub(crate) fn resumed_comms_name_matches_current_or_legacy(current: &str, stored: &str) -> bool {
    let Some((current_mob, current_role, current_member)) = split_member_comms_name(current) else {
        return false;
    };
    let Some((stored_mob, stored_role, stored_member)) = split_member_comms_name(stored) else {
        return false;
    };
    current_mob == stored_mob
        && current_role == stored_role
        && resumed_member_segment_matches_current_or_legacy(current_member, stored_member)
}

fn resumed_member_binding_matches_current_or_legacy(
    current: &meerkat_core::MobMemberBinding,
    stored: &meerkat_core::MobMemberBinding,
) -> bool {
    current.mob_id == stored.mob_id
        && current.role == stored.role
        && resumed_member_segment_matches_current_or_legacy(&current.member, &stored.member)
}

fn resumed_member_segment_matches_current_or_legacy(current: &str, stored: &str) -> bool {
    if current == stored {
        return true;
    }
    if current
        .strip_prefix("mk--")
        .map(decode_legacy_member_alias_segment)
        .is_some_and(|legacy| stored == legacy)
    {
        return true;
    }
    mobkit_generation_zero_identity_runtime_alias(current).is_some_and(|alias| stored == alias)
}

fn mobkit_generation_zero_identity_runtime_alias(current: &str) -> Option<String> {
    let mut encoded = String::with_capacity(current.len() + 26);
    encoded.push_str("mk--rt_cidentity_c");
    for character in current.chars() {
        match character {
            '_' => encoded.push_str("__"),
            ':' => encoded.push_str("_c"),
            character if character.is_ascii_alphanumeric() || character == '-' => {
                encoded.push(character);
            }
            _ => return None,
        }
    }
    encoded.push_str("_c0");
    Some(encoded)
}

fn split_member_comms_name(value: &str) -> Option<(&str, &str, &str)> {
    let mut parts = value.split('/');
    let (Some(mob_id), Some(role), Some(member), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some((mob_id, role, member))
}

pub(crate) fn legacy_raw_alias_comms_name(current: &str) -> Option<String> {
    let (mob_id, role, member) = split_member_comms_name(current)?;
    let legacy_member = member
        .strip_prefix("mk--")
        .map(decode_legacy_member_alias_segment)?;
    Some(format!("{mob_id}/{role}/{legacy_member}"))
}

fn decode_legacy_member_alias_segment(encoded: &str) -> String {
    let mut decoded = String::with_capacity(encoded.len());
    let mut chars = encoded.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '_' && chars.peek() == Some(&'c') {
            chars.next();
            decoded.push(':');
        } else {
            decoded.push(ch);
        }
    }
    decoded
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
        injected_context: Vec::new(),
        model: config.model.clone(),
        prompt,
        system_prompt: config.system_prompt.clone(),
        max_tokens: config.max_tokens,
        event_tx: None,

        // Mob runtime owns lifecycle startup and starts autonomous host loops
        // explicitly after provisioning. Avoid synchronous first-turn execution
        // during create_session so spawn does not block on LLM latency, and do
        // not stage the kickoff prompt here because the runtime will send it
        // explicitly on the first real turn.
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(build_options),
        labels: config.peer_meta.as_ref().map(|meta| meta.labels.clone()),
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
    use async_trait::async_trait;
    use meerkat_client::{LlmClient, LlmEvent, LlmRequest, TestClient};
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct CaptureClient {
        inner: TestClient,
        seen_tools: Mutex<Vec<String>>,
    }

    impl CaptureClient {
        fn tool_names(&self) -> Vec<String> {
            self.seen_tools.lock().expect("capture lock").clone()
        }
    }

    #[async_trait]
    impl LlmClient for CaptureClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
        > {
            *self.seen_tools.lock().expect("capture lock") = request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect();
            self.inner.stream(request)
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.inner.provider()
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            self.inner.health_check().await
        }
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            ProfileBinding::Inline(Box::new(Profile {
                model: "claude-opus-4-8".into(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: vec!["leader-skill".into()],
                tools: ToolConfig {
                    builtins: true,
                    shell: true,
                    comms: true,
                    memory: false,
                    workgraph: true,
                    mob: true,
                    schedule: false,
                    image_generation: true,
                    mcp: vec![],
                    mcp_servers: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "Orchestrates the mob".into(),
                external_addressable: true,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Box::new(Profile {
                model: "claude-sonnet-4-5".into(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: vec![],
                tools: ToolConfig {
                    builtins: true,
                    shell: false,
                    comms: true,
                    memory: false,
                    workgraph: false,
                    mob: false,
                    schedule: false,
                    image_generation: false,
                    mcp: vec![],
                    mcp_servers: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "Does work".into(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );

        let mut skills = BTreeMap::new();
        skills.insert(
            "leader-skill".into(),
            SkillSource::Inline {
                content: "You are the team lead.".into(),
            },
        );

        let mut definition = MobDefinition::explicit("test-mob");
        definition.orchestrator = Some(OrchestratorConfig {
            profile: ProfileName::from("lead"),
        });
        definition.profiles = profiles;
        definition.skills = skills;
        definition
    }

    fn injected_authority() -> Option<MobToolAuthorityContext> {
        let authority =
            meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority().ok()?;
        meerkat_runtime::mob_operator_authority::grant_manage_mob(&authority, "test-mob").ok()
    }

    struct TestVisibilityToolDispatcher {
        tools: Arc<[Arc<meerkat_core::types::ToolDef>]>,
    }

    #[async_trait]
    impl meerkat_core::AgentToolDispatcher for TestVisibilityToolDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::not_found(call.name))
        }
    }

    struct CaptureSnapshotMobFactory {
        captured: Arc<Mutex<Option<meerkat_core::service::MobToolSnapshotContext>>>,
    }

    #[async_trait]
    impl meerkat_core::service::MobToolsFactory for CaptureSnapshotMobFactory {
        async fn build_mob_tools(
            &self,
            args: meerkat_core::service::MobToolsBuildArgs,
        ) -> Result<
            Arc<dyn meerkat_core::AgentToolDispatcher>,
            Box<dyn std::error::Error + Send + Sync>,
        > {
            *self.captured.lock().expect("snapshot capture lock") =
                Some(args.snapshot_context.clone());
            Ok(Arc::new(TestVisibilityToolDispatcher {
                tools: Arc::from(Vec::<Arc<meerkat_core::types::ToolDef>>::new()),
            }))
        }
    }

    async fn parent_composition_authority_for_tools(
        tools: Vec<Arc<meerkat_core::types::ToolDef>>,
    ) -> meerkat_core::service::ParentToolCompositionAuthority {
        let captured = Arc::new(Mutex::new(None));
        let mob_factory = Arc::new(CaptureSnapshotMobFactory {
            captured: Arc::clone(&captured),
        });
        let temp = tempfile::tempdir().expect("temp agent factory dir");
        let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .mob(true)
            .mob_tools_factory(mob_factory.clone());
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(meerkat_core::Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.override_mob = ToolCategoryOverride::Enable;
        build.mob_tool_authority_context = injected_authority();
        build.mob_tools = Some(mob_factory);
        build.tool_dispatcher_override = Some(Arc::new(TestVisibilityToolDispatcher {
            tools: Arc::from(tools),
        }));

        let agent = factory
            .build_agent(build, &meerkat_core::Config::default())
            .await
            .expect("agent build should mint parent tool composition authority");
        let context = captured
            .lock()
            .expect("snapshot capture lock")
            .take()
            .expect("mob factory should capture snapshot context");
        let _agent = Box::leak(Box::new(agent));
        match context {
            meerkat_core::service::MobToolSnapshotContext::ParentOwned(authority) => authority,
            meerkat_core::service::MobToolSnapshotContext::Standalone => {
                panic!("mob factory should receive parent-owned snapshot authority")
            }
        }
    }

    fn test_visibility_tool(
        name: &str,
        with_provenance: bool,
    ) -> Arc<meerkat_core::types::ToolDef> {
        let tool = meerkat_core::types::ToolDef::new(
            name,
            format!("test tool {name}"),
            serde_json::json!({ "type": "object" }),
        );
        Arc::new(if with_provenance {
            tool.with_provenance(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Callback,
                source_id: name.into(),
            })
        } else {
            tool
        })
    }

    async fn witnessed_filter(
        filter: meerkat_core::tool_scope::ToolFilter,
        names: &[&str],
    ) -> InheritedToolVisibilityAuthority {
        let authority = parent_composition_authority_for_tools(
            names
                .iter()
                .map(|name| test_visibility_tool(name, true))
                .collect(),
        )
        .await;
        authority
            .authorize_inherited_tool_visibility(filter)
            .expect("test visibility authority should mint from snapshot")
    }

    fn test_workgraph_tools() -> Arc<dyn meerkat_core::AgentToolDispatcher> {
        Arc::new(meerkat::WorkGraphToolSurface::new(
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new())),
        ))
    }

    fn resumed_session_with_metadata(session_id: SessionId) -> Session {
        let mut resumed_session = Session::with_id(session_id);
        resumed_session
            .set_session_metadata(SessionMetadata {
                schema_version: meerkat_core::session_metadata_schema_version(),
                model: "claude-opus-4-8".to_string(),
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
                    schedule: meerkat_core::session::ToolCategoryOverride::Enable,
                    workgraph: meerkat_core::session::ToolCategoryOverride::Enable,
                    image_generation: meerkat_core::session::ToolCategoryOverride::Enable,
                    web_search: meerkat_core::session::ToolCategoryOverride::Inherit,
                    tool_access_policy: None,
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
                mob_member_binding: Some(meerkat_core::MobMemberBinding {
                    mob_id: "test-mob".to_string(),
                    role: "lead".to_string(),
                    member: "lead-1".to_string(),
                }),
            })
            .expect("session metadata");
        resumed_session
    }

    fn session_with_visibility_state_for_test(
        session: Session,
        state: SessionToolVisibilityState,
    ) -> Session {
        let mut raw_session = serde_json::to_value(session).expect("session should serialize");
        raw_session
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .expect("session metadata should be an object")
            .insert(
                meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY.to_string(),
                serde_json::to_value(state).expect("visibility state should serialize"),
            );
        serde_json::from_value(raw_session).expect("visibility-state session fixture")
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert!(!config.keep_alive, "keep_alive must be false for mob spawn");
    }

    /// M2 (d): profile-declared MCP servers are DURABLE build inputs — they
    /// must thread from `ProfileTools.mcp_servers` into
    /// `AgentBuildConfig.mcp_servers` so member revival (which re-resolves
    /// the same profile) recomposes the member's MCP tools instead of losing
    /// them like the in-process-only per-spawn overlay did.
    #[tokio::test]
    async fn test_build_agent_config_threads_profile_mcp_servers() {
        let def = sample_definition();
        let mut profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap()
            .clone();
        profile.tools.mcp_servers = vec![meerkat_core::mcp_config::McpServerConfig::stdio(
            "planner",
            "/bin/echo",
            Vec::<String>::new(),
            std::collections::HashMap::new(),
        )];
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: &profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.mcp_servers.len(), 1);
        assert_eq!(config.mcp_servers[0].name, "planner");
    }

    /// Spawn-site threading: the spec's `tool_access_policy` must reach
    /// `AgentBuildConfig.tool_access_policy` (the seam the factory gates on).
    #[tokio::test]
    async fn test_build_agent_config_threads_tool_access_policy() {
        let def = sample_definition();
        let profile = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let profile_name = ProfileName::from("lead");
        let agent_identity = AgentIdentity::from("lead-1");
        let params = |policy: Option<meerkat_core::ops::ToolAccessPolicy>| BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &profile_name,
            agent_identity: &agent_identity,
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: policy,
            inherited_tool_filter: None,
            system_prompt_override: None,
        };

        let allow = meerkat_core::ops::ToolAccessPolicy::AllowList(
            ["read_file", "send_message"].into_iter().collect(),
        );
        let config = build_agent_config(params(Some(allow.clone())))
            .await
            .expect("build_agent_config");
        assert_eq!(
            config.tool_access_policy,
            Some(allow),
            "explicit spec policy must ride into AgentBuildConfig"
        );

        let deny = meerkat_core::ops::ToolAccessPolicy::DenyList(["bash"].into_iter().collect());
        let config = build_agent_config(params(Some(deny.clone())))
            .await
            .expect("build_agent_config");
        assert_eq!(config.tool_access_policy, Some(deny));

        // Host/operator authority: `Inherit` reaching this compiler resolves
        // to unrestricted (agent-facing surfaces resolve Inherit to the
        // parent's persisted policy before the spec reaches the actor). An
        // `Inherit` leaking into `AgentBuildConfig` would fail the factory
        // build closed instead.
        let config = build_agent_config(params(Some(meerkat_core::ops::ToolAccessPolicy::Inherit)))
            .await
            .expect("build_agent_config");
        assert_eq!(config.tool_access_policy, None);

        let config = build_agent_config(params(None))
            .await
            .expect("build_agent_config");
        assert_eq!(config.tool_access_policy, None);
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.comms_name.as_deref(),
            Some("test-mob/lead/lead-1"),
            "comms_name should be mob_id/profile/meerkat_id"
        );
        assert_eq!(
            config.mob_member_binding,
            Some(meerkat_core::MobMemberBinding {
                mob_id: "test-mob".to_string(),
                role: "lead".to_string(),
                member: "lead-1".to_string(),
            }),
            "producer must stamp the typed durable mob member binding"
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
            agent_identity: &AgentIdentity::from("w-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
        assert_eq!(
            meta.labels.get("agent_identity").map(String::as_str),
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.realm_id.as_ref().map(meerkat_core::RealmId::as_str),
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            config.override_workgraph,
            meerkat_core::ToolCategoryOverride::Enable
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
        let authority = config
            .mob_tool_authority_context
            .as_ref()
            .expect("mob profile should receive generated authority");
        assert!(authority.spawn_profile_scope_contains("test-mob", "lead"));
        assert!(authority.spawn_profile_scope_contains("test-mob", "worker"));
        assert!(!authority.can_manage_mob("test-mob"));
        // Worker profile has builtins=true, shell=false, memory=false
        let worker = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &AgentIdentity::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            config.override_workgraph,
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
    async fn profile_workgraph_does_not_displace_builtin_task_tools_in_agent_build() {
        let temp = tempfile::tempdir().unwrap();
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let capture = Arc::new(CaptureClient::default());
        let mut config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");
        config.llm_client_override = Some(capture.clone());
        config.workgraph_tools = Some(test_workgraph_tools());

        let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .workgraph(false);
        let mut agent = factory
            .build_agent(config, &meerkat_core::Config::default())
            .await
            .expect("build agent from mob profile");
        agent
            .run("inspect tools".to_string().into())
            .await
            .expect("run agent");

        let tool_names = capture.tool_names();
        assert!(
            tool_names.iter().any(|name| name == "task_create"),
            "profile tools.builtins=true must keep builtin task tools visible; saw {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|name| name == "task_list"),
            "profile tools.builtins=true must keep builtin task list visible; saw {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|name| name == "workgraph_create"),
            "profile tools.workgraph=true must expose WorkGraph tools; saw {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|name| name == "workgraph_ready"),
            "profile tools.workgraph=true must expose WorkGraph readiness; saw {tool_names:?}"
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
            witnessed_filter(inherited_filter.clone(), &["bash", "mob_spawn_member"]).await;
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &AgentIdentity::from("w-inherit"),
            profile: &profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: Some(inherited_authority),
            system_prompt_override: None,
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
            config.override_workgraph,
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
        let inherited_authority = witnessed_filter(inherited_filter.clone(), &["shell"]).await;
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &AgentIdentity::from("lead-1"),
            profile,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: Some(inherited_authority.clone()),
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert!(
            !config
                .initial_metadata_entries
                .contains_key(meerkat_core::tool_scope::INHERITED_TOOL_FILTER_METADATA_KEY),
            "mob build must not write legacy inherited visibility metadata"
        );
        assert!(
            !config
                .initial_metadata_entries
                .contains_key(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY),
            "mob build must not write canonical visibility as raw metadata"
        );
        let visibility_state = config
            .initial_tool_visibility_state
            .as_ref()
            .expect("typed visibility handoff should be present");
        assert_eq!(
            visibility_state.filter(),
            &inherited_filter,
            "inherited mob filter should flow through canonical visibility state"
        );
        assert_eq!(
            visibility_state.witnesses(),
            inherited_authority.witnesses(),
            "inherited mob filter witnesses should flow through canonical visibility state"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_rejects_name_only_inherited_filter() {
        let authority =
            parent_composition_authority_for_tools(vec![test_visibility_tool("shell", false)])
                .await;
        let err = authority
            .authorize_inherited_tool_visibility(meerkat_core::tool_scope::ToolFilter::Allow(
                ["shell".to_string()].into_iter().collect(),
            ))
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
            agent_identity: &AgentIdentity::from("lead-operator"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: injected_authority(),
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
                agent_identity: &AgentIdentity::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                tool_access_policy: None,
                inherited_tool_filter: None,
                system_prompt_override: None,
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
    async fn test_build_resumed_agent_config_preserves_persisted_schedule_and_workgraph_overrides()
    {
        let def = sample_definition();
        let mut lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap()
            .clone();
        lead.tools.workgraph = false;
        lead.tools.schedule = false;
        let session_id = SessionId::new();
        let resumed_session = resumed_session_with_metadata(session_id.clone());

        let config = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &ProfileName::from("lead"),
                agent_identity: &AgentIdentity::from("lead-1"),
                profile: &lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                tool_access_policy: None,
                inherited_tool_filter: None,
                system_prompt_override: None,
            },
            expected_session_id: &session_id,
            resumed_session,
        })
        .await
        .expect("build_resumed_agent_config");

        assert_eq!(
            config.override_schedule,
            meerkat_core::ToolCategoryOverride::Enable,
            "resumed mob members must keep durable schedule exposure intent from metadata"
        );
        assert_eq!(
            config.override_workgraph,
            meerkat_core::ToolCategoryOverride::Enable,
            "resumed mob members must keep durable WorkGraph exposure intent from metadata"
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
        let resumed_session = resumed_session_with_metadata(session_id.clone());
        let inherited_filter = meerkat_core::tool_scope::ToolFilter::Deny(
            ["parent_shell".to_string()].into_iter().collect(),
        );
        let inherited_authority =
            witnessed_filter(inherited_filter.clone(), &["parent_shell"]).await;
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
            active_requested_deferred_names: BTreeSet::from(["deferred_active".into()]),
            staged_requested_deferred_names: BTreeSet::from(["deferred_staged".into()]),
            active_revision: 7,
            staged_revision: 9,
            requested_witnesses: [(
                "deferred_active".into(),
                meerkat_core::ToolVisibilityWitness {
                    last_seen_provenance: Some(meerkat_core::ToolProvenance {
                        kind: meerkat_core::ToolSourceKind::Callback,
                        source_id: "deferred_active".into(),
                    }),
                },
            )]
            .into_iter()
            .collect(),
            filter_witnesses: [(
                "active_secret".into(),
                meerkat_core::ToolVisibilityWitness {
                    last_seen_provenance: Some(meerkat_core::ToolProvenance {
                        kind: meerkat_core::ToolSourceKind::Callback,
                        source_id: "active_secret".into(),
                    }),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let resumed_session =
            session_with_visibility_state_for_test(resumed_session, original_state.clone());

        let config = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &ProfileName::from("lead"),
                agent_identity: &AgentIdentity::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                tool_access_policy: None,
                inherited_tool_filter: Some(inherited_authority.clone()),
                system_prompt_override: None,
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
            "resumed inherited visibility must not use raw metadata handoff"
        );
        let inherited_visibility_state = config
            .initial_tool_visibility_state
            .as_ref()
            .expect("resumed inherited visibility handoff should be present");
        assert_eq!(inherited_visibility_state.filter(), &inherited_filter);
        assert_eq!(
            inherited_visibility_state.witnesses(),
            inherited_authority.witnesses()
        );
        let visibility_state = config
            .resume_session
            .as_ref()
            .expect("resume session")
            .try_tool_visibility_state()
            .expect("parse visibility")
            .expect("visibility state");
        assert_eq!(
            visibility_state.inherited_base_filter,
            original_state.inherited_base_filter
        );
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
        assert_eq!(
            visibility_state.filter_witnesses,
            original_state.filter_witnesses
        );
    }

    #[tokio::test]
    async fn test_build_resumed_agent_config_rejects_malformed_visibility_state_before_merge() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let session_id = SessionId::new();
        let resumed_session = resumed_session_with_metadata(session_id.clone());
        let mut raw_session =
            serde_json::to_value(&resumed_session).expect("session should serialize");
        raw_session["metadata"][meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY] =
            serde_json::json!("not-a-visibility-state");
        let resumed_session: Session =
            serde_json::from_value(raw_session).expect("raw malformed session fixture");
        let inherited_authority = witnessed_filter(
            meerkat_core::tool_scope::ToolFilter::Deny(
                ["parent_shell".to_string()].into_iter().collect(),
            ),
            &["parent_shell"],
        )
        .await;

        let err = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &ProfileName::from("lead"),
                agent_identity: &AgentIdentity::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                tool_access_policy: None,
                inherited_tool_filter: Some(inherited_authority),
                system_prompt_override: None,
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
            agent_identity: &AgentIdentity::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        let prompt = config
            .system_prompt
            .as_set_prompt()
            .expect("system_prompt set");
        assert!(
            prompt.contains("You are the team lead."),
            "prompt should contain resolved inline skill"
        );
        // Mob comms instructions are delivered via preload_skills (embedded
        // skill), not baked into system_prompt — verified in the separate
        // test_build_agent_config_preloads_mob_communication_skill test.
    }

    #[tokio::test]
    async fn test_build_agent_config_system_prompt_replace_keeps_mob_skill_preload() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: Some(crate::SpawnSystemPromptOverride::Replace(
                "OB3 replacement prompt".to_string(),
            )),
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.system_prompt.as_set_prompt(),
            Some("OB3 replacement prompt"),
            "typed Replace must bypass profile prompt assembly"
        );
        assert!(
            config.preload_skills.as_ref().is_some_and(|skills| skills
                .iter()
                .any(|skill| skill.to_string().contains("mob-communication"))),
            "prompt replacement must not remove required mob runtime skill wiring"
        );
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
        assert!(
            preload
                .iter()
                .any(|id| id.skill_name.as_str() == "workgraph-workflow"),
            "WorkGraph-capable profiles should preload WorkGraph operating rules"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_preloads_task_workflow_when_workgraph_absent() {
        let def = sample_definition();
        let worker = def.profiles[&ProfileName::from("worker")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("worker"),
            agent_identity: &AgentIdentity::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
                .any(|id| id.skill_name.as_str() == "task-workflow"),
            "builtin-only task-capable profiles should preload local task operating rules"
        );
        assert!(
            !preload
                .iter()
                .any(|id| id.skill_name.as_str() == "workgraph-workflow"),
            "profiles without WorkGraph should not preload WorkGraph operating rules"
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.model, "claude-opus-4-8");
    }

    #[tokio::test]
    async fn test_to_create_session_request() {
        let def = sample_definition();
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let mut labels = BTreeMap::new();
        labels.insert("faction".to_string(), "north".to_string());
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: Some(labels),
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        let req = to_create_session_request(&config, "Hello mob".to_string().into());
        assert_eq!(req.model, "claude-opus-4-8");
        assert_eq!(req.prompt.text_content(), "Hello mob");
        assert!(req.system_prompt.is_explicit());
        assert_eq!(
            req.initial_turn,
            meerkat_core::service::InitialTurnPolicy::Defer
        );
        assert_eq!(req.deferred_prompt_policy, DeferredPromptPolicy::Discard);
        {
            let labels = req.labels.as_ref().expect("request labels");
            assert_eq!(labels.get("mob_id").map(String::as_str), Some("test-mob"));
            assert_eq!(labels.get("role").map(String::as_str), Some("lead"));
            assert_eq!(labels.get("meerkat_id").map(String::as_str), Some("lead-1"));
            assert_eq!(
                labels.get("agent_identity").map(String::as_str),
                Some("lead-1")
            );
            assert_eq!(labels.get("faction").map(String::as_str), Some("north"));
        }

        let build = req.build.expect("build options should be set");
        assert_eq!(build.comms_name.as_deref(), Some("test-mob/lead/lead-1"));
        assert!(build.peer_meta.is_some());
        assert_eq!(
            build.realm_id.as_ref().map(meerkat_core::RealmId::as_str),
            Some("mob.test-mob")
        );
        assert_eq!(
            build.mob_member_binding,
            Some(meerkat_core::MobMemberBinding {
                mob_id: "test-mob".to_string(),
                role: "lead".to_string(),
                member: "lead-1".to_string(),
            }),
            "typed durable member binding must flow into SessionBuildOptions"
        );
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
            agent_identity: &AgentIdentity::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
    async fn test_to_create_session_request_preserves_resumed_labels() {
        let def = sample_definition();
        let lead_key = ProfileName::from("lead");
        let lead = def.profiles[&lead_key].as_inline().unwrap();
        let session_id = SessionId::new();
        let mut resumed_session = resumed_session_with_metadata(session_id.clone());
        let mut persisted_meta = resumed_session
            .session_metadata()
            .expect("session metadata");
        persisted_meta.peer_meta = Some(
            PeerMeta::default()
                .with_label("mob_id", "test-mob")
                .with_label("role", "lead")
                .with_label("meerkat_id", "lead-1"),
        );
        resumed_session
            .set_session_metadata(persisted_meta)
            .expect("session metadata");

        let config = build_resumed_agent_config(BuildResumedAgentConfigParams {
            base: BuildAgentConfigParams {
                mob_id: &def.id,
                profile_name: &lead_key,
                agent_identity: &AgentIdentity::from("lead-1"),
                profile: lead,
                definition: &def,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_authority_context: None,
                tool_access_policy: None,
                inherited_tool_filter: None,
                system_prompt_override: None,
            },
            expected_session_id: &session_id,
            resumed_session,
        })
        .await
        .expect("build_resumed_agent_config");

        let req = to_create_session_request(&config, "Resume mob".to_string().into());
        let labels = req.labels.as_ref().expect("request labels");
        assert_eq!(labels.get("mob_id").map(String::as_str), Some("test-mob"));
        assert_eq!(labels.get("role").map(String::as_str), Some("lead"));
        assert_eq!(labels.get("meerkat_id").map(String::as_str), Some("lead-1"));
        assert_eq!(
            labels.get("agent_identity").map(String::as_str),
            Some("lead-1")
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config should resolve path skill");

        let prompt = config.system_prompt.as_set_prompt().expect("system prompt");
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: Some(ctx.clone()),
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
            .provider_params = Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(
                    meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
                        thinking_budget: Some(4096),
                        top_k: Some(40),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        );

        let lead = def.profiles[&lead_key].as_inline().unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &lead_key,
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(
            config.provider_params,
            Some(
                meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                    provider_tag: Some(
                        meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(
                            meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
                                thinking_budget: Some(4096),
                                top_k: Some(40),
                                ..Default::default()
                            },
                        )
                    ),
                    ..Default::default()
                }
            ),
            "provider_params should be passed through to AgentBuildConfig"
        );

        let req = to_create_session_request(&config, "hello".to_string().into());
        let build = req.build.expect("build options should be set");
        assert_eq!(
            build.provider_params,
            Some(
                meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                    provider_tag: Some(
                        meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(
                            meerkat_core::lifecycle::run_primitive::GeminiProviderTag {
                                thinking_budget: Some(4096),
                                top_k: Some(40),
                                ..Default::default()
                            },
                        )
                    ),
                    ..Default::default()
                }
            ),
            "provider_params should survive into SessionBuildOptions"
        );
    }

    #[tokio::test]
    async fn test_build_agent_config_maps_profile_provider_and_definition_models() {
        let mut def = sample_definition();
        def.models.insert(
            "claude-internal-preview".to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: meerkat_core::Provider::Anthropic,
                display_name: None,
                context_window: Some(500_000),
                max_output_tokens: None,
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        def.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let lead_key = ProfileName::from("lead");
        {
            let lead = def
                .profiles
                .get_mut(&lead_key)
                .expect("lead profile")
                .as_inline_mut()
                .unwrap();
            lead.model = "claude-internal-preview".to_string();
            lead.provider = Some(meerkat_core::Provider::Anthropic);
            lead.self_hosted_server_id = None;
            lead.image_generation_provider = Some(meerkat_core::Provider::Gemini);
            lead.auto_compact_threshold = std::num::NonZeroU64::new(60_000);
            lead.resume_overrides = vec![
                crate::profile::ResumeOverrideField::Model,
                crate::profile::ResumeOverrideField::Provider,
            ];
        }

        let lead = def.profiles[&lead_key].as_inline().unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &lead_key,
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");

        assert_eq!(config.provider, Some(meerkat_core::Provider::Anthropic));
        assert_eq!(
            config
                .custom_models
                .get("claude-internal-preview")
                .map(|model| model.provider),
            Some(meerkat_core::Provider::Anthropic),
            "definition [models.<id>] entries must ride the build config"
        );
        assert_eq!(
            config.image_generation_provider,
            Some(meerkat_core::Provider::Gemini),
            "profile-level image_generation_provider wins over the mob-level default"
        );
        assert_eq!(
            config.auto_compact_threshold_override,
            std::num::NonZeroU64::new(60_000)
        );
        assert!(config.resume_override_mask.model);
        assert!(config.resume_override_mask.provider);
        assert!(!config.resume_override_mask.provider_params);

        // The seam survives into SessionBuildOptions (deferred materialization).
        let req = to_create_session_request(&config, "hello".to_string().into());
        let build = req.build.expect("build options should be set");
        assert_eq!(build.provider, Some(meerkat_core::Provider::Anthropic));
        assert!(build.custom_models.contains_key("claude-internal-preview"));
        assert_eq!(
            build.image_generation_provider,
            Some(meerkat_core::Provider::Gemini)
        );
        assert_eq!(
            build.auto_compact_threshold_override,
            std::num::NonZeroU64::new(60_000)
        );
        assert!(build.resume_override_mask.model);
    }

    #[tokio::test]
    async fn test_build_agent_config_uses_mob_level_image_provider_default() {
        let mut def = sample_definition();
        def.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let lead = def.profiles[&ProfileName::from("lead")]
            .as_inline()
            .unwrap();
        let config = build_agent_config(BuildAgentConfigParams {
            mob_id: &def.id,
            profile_name: &ProfileName::from("lead"),
            agent_identity: &AgentIdentity::from("lead-1"),
            profile: lead,
            definition: &def,
            external_tools: None,
            context: None,
            labels: None,
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
        })
        .await
        .expect("build_agent_config");
        assert_eq!(
            config.image_generation_provider,
            Some(meerkat_core::Provider::OpenAI),
            "mob-level image_generation_provider applies when the profile is silent"
        );
    }

    #[test]
    fn test_resumed_metadata_respects_profile_resume_overrides() {
        // Profile says model+provider win on resume; metadata carries stale
        // identity. Masked fields keep profile truth, unmasked fields restore
        // durable truth.
        let mut config = AgentBuildConfig::new("claude-opus-4-8");
        config.provider = Some(meerkat_core::Provider::Anthropic);
        config.comms_name = Some("mob.m/lead/lead-1".to_string());
        let fresh_params = meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            temperature: Some(0.1),
            ..Default::default()
        };
        let stale_params = meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            temperature: Some(0.9),
            ..Default::default()
        };
        config.provider_params = Some(fresh_params);
        config.resume_override_mask.model = true;
        config.resume_override_mask.provider = true;

        let durable_auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("durable").unwrap(),
            binding: meerkat_core::BindingId::parse("durable_primary").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let metadata = SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "gpt-5.4".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: Some(stale_params.clone()),
            tooling: Default::default(),
            keep_alive: false,
            comms_name: Some("mob.m/lead/lead-1".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: Some(durable_auth_binding.clone()),
            mob_member_binding: None,
        };

        apply_resumed_session_metadata(&mut config, &metadata).expect("metadata applies");

        assert_eq!(
            config.model, "claude-opus-4-8",
            "masked model keeps the (possibly updated) profile value"
        );
        assert_eq!(
            config.provider,
            Some(meerkat_core::Provider::Anthropic),
            "masked provider keeps the profile value"
        );
        assert_eq!(
            config.provider_params,
            Some(stale_params),
            "unmasked provider_params restore durable truth"
        );
        assert_eq!(
            config.auth_binding.as_ref(),
            Some(&durable_auth_binding),
            "an omitted current binding restores the durable configured binding"
        );

        // Without the mask, durable metadata wins (existing behavior pinned).
        let mut config = AgentBuildConfig::new("claude-opus-4-8");
        config.comms_name = Some("mob.m/lead/lead-1".to_string());
        apply_resumed_session_metadata(&mut config, &metadata).expect("metadata applies");
        assert_eq!(config.model, "gpt-5.4");
        assert_eq!(config.provider, Some(meerkat_core::Provider::OpenAI));

        let explicit_auth_binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("updated").unwrap(),
            binding: meerkat_core::BindingId::parse("openai_primary").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let mut explicit = AgentBuildConfig::new("gpt-5.4");
        explicit.comms_name = Some("mob.m/lead/lead-1".to_string());
        explicit.auth_binding = Some(explicit_auth_binding.clone());
        apply_resumed_session_metadata(&mut explicit, &metadata).expect("metadata applies");
        assert_eq!(
            explicit.auth_binding.as_ref(),
            Some(&explicit_auth_binding),
            "an explicit current binding keeps precedence over durable metadata"
        );
    }

    #[test]
    fn test_resumed_metadata_accepts_legacy_raw_alias_comms_name() {
        let mut config = AgentBuildConfig::new("gpt-5.4");
        config.comms_name = Some("ob3/review/mk--rt_creview_csingleton_c0".to_string());

        let metadata = SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "gpt-5.4".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: Default::default(),
            keep_alive: false,
            comms_name: Some("ob3/review/rt:review:singleton:0".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        };

        apply_resumed_session_metadata(&mut config, &metadata).expect("legacy metadata applies");
        assert_eq!(
            config.comms_name.as_deref(),
            Some("ob3/review/mk--rt_creview_csingleton_c0"),
            "resume should rehydrate to the canonical encoded comms name"
        );
    }

    #[test]
    fn test_resumed_metadata_accepts_exact_identity_runtime_alias() {
        let canonical_binding = meerkat_core::MobMemberBinding {
            mob_id: "homecore".to_string(),
            role: "identity".to_string(),
            member: "parent-1".to_string(),
        };
        let mut config = AgentBuildConfig::new("gpt-5.5");
        config.comms_name = Some("homecore/identity/parent-1".to_string());
        config.mob_member_binding = Some(canonical_binding.clone());

        let legacy_alias = "mk--rt_cidentity_cparent-1_c0";
        let metadata = SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "gpt-5.5".to_string(),
            max_tokens: 16_384,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: Default::default(),
            keep_alive: false,
            comms_name: Some(format!("homecore/identity/{legacy_alias}")),
            peer_meta: Some(
                PeerMeta::default()
                    .with_label("fixture", "retained")
                    .with_label("agent_identity", legacy_alias)
                    .with_label("meerkat_id", legacy_alias),
            ),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "homecore".to_string(),
                role: "identity".to_string(),
                member: legacy_alias.to_string(),
            }),
        };

        apply_resumed_session_metadata(&mut config, &metadata)
            .expect("exact generation-zero identity runtime alias applies");
        assert_eq!(
            config.comms_name.as_deref(),
            Some("homecore/identity/parent-1")
        );
        assert_eq!(config.mob_member_binding, Some(canonical_binding.clone()));
        let labels = &config.peer_meta.expect("canonical peer metadata").labels;
        assert_eq!(labels.get("fixture").map(String::as_str), Some("retained"));
        assert_eq!(
            labels.get("agent_identity").map(String::as_str),
            Some("parent-1")
        );
        assert_eq!(
            labels.get("meerkat_id").map(String::as_str),
            Some("parent-1")
        );

        // The run-boundary projection may publish the typed canonical binding
        // before replacing the legacy transport alias. Both observations
        // independently prove the same current member and must remain a valid
        // input to the next level-triggered resume.
        let mut partial_projection = metadata;
        partial_projection.mob_member_binding = Some(canonical_binding.clone());
        let mut replay_config = AgentBuildConfig::new("gpt-5.5");
        replay_config.comms_name = Some("homecore/identity/parent-1".to_string());
        replay_config.mob_member_binding = Some(canonical_binding);
        apply_resumed_session_metadata(&mut replay_config, &partial_projection)
            .expect("canonical binding plus predecessor comms alias remains recoverable");
    }

    #[test]
    fn test_identity_runtime_alias_rejects_wrong_mob_role_member_or_generation() {
        let current = "homecore/identity/parent-1";
        for stored in [
            "other/identity/mk--rt_cidentity_cparent-1_c0",
            "homecore/worker/mk--rt_cidentity_cparent-1_c0",
            "homecore/identity/mk--rt_cidentity_cparent-2_c0",
            "homecore/identity/mk--rt_cworker_cparent-1_c0",
            "homecore/identity/mk--rt_cidentity_cparent-1_c1",
        ] {
            assert!(
                !resumed_comms_name_matches_current_or_legacy(current, stored),
                "unproven legacy alias {stored} must fail closed"
            );
        }
    }

    #[test]
    fn test_resumed_metadata_rejects_tampered_binding_even_with_valid_comms_alias() {
        let mut config = AgentBuildConfig::new("gpt-5.5");
        config.comms_name = Some("homecore/identity/parent-1".to_string());
        config.mob_member_binding = Some(meerkat_core::MobMemberBinding {
            mob_id: "homecore".to_string(),
            role: "identity".to_string(),
            member: "parent-1".to_string(),
        });
        let metadata = SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "gpt-5.5".to_string(),
            max_tokens: 16_384,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: Default::default(),
            keep_alive: false,
            comms_name: Some("homecore/identity/mk--rt_cidentity_cparent-1_c0".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "homecore".to_string(),
                role: "identity".to_string(),
                member: "mk--rt_cidentity_cparent-1_c1".to_string(),
            }),
        };

        let error = apply_resumed_session_metadata(&mut config, &metadata)
            .expect_err("binding and comms alias must identify the same legacy runtime");
        assert!(
            error.to_string().contains("persisted mob member binding"),
            "unexpected refusal: {error}"
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
            agent_identity: &AgentIdentity::from("w-1"),
            profile: worker,
            definition: &def,
            external_tools: None,
            context: None,
            labels: Some(app_labels),
            additional_instructions: None,
            shell_env: None,
            mob_tool_authority_context: None,
            tool_access_policy: None,
            inherited_tool_filter: None,
            system_prompt_override: None,
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
        assert_eq!(
            meta.labels.get("agent_identity").map(String::as_str),
            Some("w-1")
        );
    }
}
