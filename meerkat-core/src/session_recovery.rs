use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::runtime_epoch::RuntimeBuildMode;
use crate::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, ResumeOverrideMask,
    SessionBuildOptions,
};
use crate::{
    AgentToolDispatcher, AuthBindingRef, BudgetLimits, ContentInput, HookRunOverrides,
    OutputSchema, PeerMeta, Provider, Session, SessionBuildState, SessionDeferredTurnState,
    SessionMetadata, ToolCategoryOverride, ToolDef, checkpoint::SessionCheckpointer,
    skills::SkillKey,
};

pub const BUILD_ONLY_RECOVERY_OVERRIDE_ERROR: &str = "Cannot override max_tokens, system_prompt, output_schema, or structured_output_retries after the deferred session's first turn has started";

/// Explicit semantic caller intent for a resumed turn.
///
/// Omitted fields inherit from [`SessionDefaults`], then from [`RealmDefaults`]
/// where a realm field has no session value. These fields are semantic agent
/// configuration; render and transport options must stay outside this type.
#[derive(Debug, Clone, Default)]
pub struct SurfaceSessionRecoveryOverrides {
    pub model: Option<String>,
    pub provider: Option<Provider>,
    pub provider_params: Option<serde_json::Value>,
    pub clear_provider_params: bool,
    pub auth_binding: Option<AuthBindingRef>,
    pub clear_auth_binding: bool,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<OutputSchema>,
    pub structured_output_retries: Option<u32>,
    pub keep_alive: Option<bool>,
    pub hooks_override: Option<HookRunOverrides>,
    pub comms_name: Option<String>,
    pub peer_meta: Option<PeerMeta>,
    pub budget_limits: Option<BudgetLimits>,
    pub override_builtins: Option<bool>,
    pub override_shell: Option<bool>,
    pub override_memory: Option<bool>,
    pub override_workgraph: Option<bool>,
    pub override_mob: Option<bool>,
    pub override_image_generation: Option<bool>,
    pub override_web_search: Option<bool>,
    pub preload_skills: Option<Vec<SkillKey>>,
    pub app_context: Option<serde_json::Value>,
    pub shell_env: Option<HashMap<String, String>>,
    pub recoverable_tool_defs: Option<Vec<ToolDef>>,
}

/// Canonical name for resumed-turn semantic overrides.
///
/// The older `SurfaceSessionRecoveryOverrides` name is kept as the public
/// compatibility spelling for existing CLI/RPC callers.
pub type TurnOverrides = SurfaceSessionRecoveryOverrides;

/// Realm/config defaults that can participate in semantic recovery.
///
/// These values come from the active realm/config context. They only fill fields
/// that are not already durable session defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealmDefaults {
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
}

impl RealmDefaults {
    #[must_use]
    pub fn from_recovery_context(context: &SurfaceSessionRecoveryContext) -> Self {
        Self {
            realm_id: context.realm_id.clone(),
            instance_id: context.instance_id.clone(),
            backend: context.backend.clone(),
            config_generation: context.config_generation,
        }
    }
}

/// Durable defaults projected from persisted session state.
#[derive(Debug, Clone)]
pub struct SessionDefaults {
    pub metadata: SessionMetadata,
    pub build_state: SessionBuildState,
    pub allows_first_turn_build_overrides: bool,
}

impl SessionDefaults {
    pub fn from_session(session: &Session) -> Result<Self, SurfaceSessionRecoveryError> {
        let metadata = session.session_metadata().ok_or_else(|| {
            SurfaceSessionRecoveryError::MissingSessionMetadata(session.id().to_string())
        })?;
        Ok(Self {
            metadata,
            build_state: session.build_state().unwrap_or_default(),
            allows_first_turn_build_overrides: session_allows_first_turn_build_overrides(session),
        })
    }
}

#[derive(Clone, Default)]
pub struct SurfaceSessionRecoveryContext {
    pub llm_client_override: Option<Arc<dyn Any + Send + Sync>>,
    pub agent_llm_client_decorator: Option<crate::AgentLlmClientDecorator>,
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    pub checkpointer: Option<Arc<dyn SessionCheckpointer>>,
    pub runtime_build_mode: Option<RuntimeBuildMode>,
    pub require_runtime_build_mode: bool,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
}

/// Resolved semantic config for the next recovered turn.
///
/// This is the internal contract for:
///
/// `EffectiveTurnConfig = RealmDefaults + SessionDefaults + TurnOverrides`
///
/// Non-semantic render/transport resources remain in
/// [`SurfaceSessionRecoveryContext`] and are copied into `build` only as host
/// plumbing.
#[derive(Debug)]
pub struct EffectiveTurnConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<u32>,
    pub keep_alive: bool,
    pub build: SessionBuildOptions,
    pub recoverable_tool_defs: Vec<ToolDef>,
}

#[derive(Debug)]
pub struct RecoveredSessionBuild {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<u32>,
    pub keep_alive: bool,
    pub build: SessionBuildOptions,
    pub recoverable_tool_defs: Vec<ToolDef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeLlmBinding {
    pub provider: Option<Provider>,
    pub self_hosted_server_id: Option<String>,
    pub provider_overridden: bool,
}

impl RecoveredSessionBuild {
    pub fn into_deferred_create_request(self) -> CreateSessionRequest {
        CreateSessionRequest {
            model: self.model,
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: self.system_prompt,
            max_tokens: self.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(self.build),
            labels: None,
        }
    }
}

impl From<EffectiveTurnConfig> for RecoveredSessionBuild {
    fn from(config: EffectiveTurnConfig) -> Self {
        Self {
            model: config.model,
            system_prompt: config.system_prompt,
            max_tokens: config.max_tokens,
            keep_alive: config.keep_alive,
            build: config.build,
            recoverable_tool_defs: config.recoverable_tool_defs,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SurfaceSessionRecoveryError {
    #[error("persisted session {0} is missing session metadata")]
    MissingSessionMetadata(String),
    #[error("{0}")]
    MissingRuntimeBuildMode(String),
    #[error("{0}")]
    InvalidOverride(String),
}

pub fn has_build_only_turn_overrides(overrides: &SurfaceSessionRecoveryOverrides) -> bool {
    overrides.max_tokens.is_some()
        || overrides.system_prompt.is_some()
        || overrides.output_schema.is_some()
        || overrides.structured_output_retries.is_some()
}

pub fn has_materialization_overrides(overrides: &SurfaceSessionRecoveryOverrides) -> bool {
    overrides.model.is_some()
        || overrides.provider.is_some()
        || overrides.provider_params.is_some()
        || overrides.clear_provider_params
        || overrides.auth_binding.is_some()
        || overrides.clear_auth_binding
        || has_build_only_turn_overrides(overrides)
        || overrides.hooks_override.is_some()
        || overrides.comms_name.is_some()
        || overrides.peer_meta.is_some()
        || overrides.budget_limits.is_some()
        || overrides.override_builtins.is_some()
        || overrides.override_shell.is_some()
        || overrides.override_memory.is_some()
        || overrides.override_mob.is_some()
        || overrides.override_image_generation.is_some()
        || overrides.override_web_search.is_some()
        || overrides.preload_skills.is_some()
        || overrides.app_context.is_some()
        || overrides.shell_env.is_some()
        || overrides.recoverable_tool_defs.is_some()
}

pub fn session_allows_first_turn_build_overrides(session: &Session) -> bool {
    session
        .deferred_turn_state()
        .as_ref()
        .is_some_and(SessionDeferredTurnState::allows_initial_turn_overrides)
}

pub fn resolve_resume_llm_binding(
    stored_provider: Provider,
    stored_self_hosted_server_id: Option<String>,
    model_override: Option<&str>,
    provider_override: Option<Provider>,
) -> ResumeLlmBinding {
    let model_changed = model_override.is_some();
    let provider_overridden = provider_override.is_some() || model_changed;
    let provider = if model_changed && provider_override.is_none() {
        None
    } else {
        Some(provider_override.unwrap_or(stored_provider))
    };
    let self_hosted_server_id = if model_changed {
        None
    } else {
        stored_self_hosted_server_id
    };

    ResumeLlmBinding {
        provider,
        self_hosted_server_id,
        provider_overridden,
    }
}

pub fn build_recovered_session(
    session: Session,
    overrides: &SurfaceSessionRecoveryOverrides,
    context: SurfaceSessionRecoveryContext,
) -> Result<RecoveredSessionBuild, SurfaceSessionRecoveryError> {
    let realm_defaults = RealmDefaults::from_recovery_context(&context);
    resolve_effective_turn_config(session, realm_defaults, overrides, context).map(Into::into)
}

pub fn resolve_effective_turn_config(
    session: Session,
    realm_defaults: RealmDefaults,
    overrides: &TurnOverrides,
    context: SurfaceSessionRecoveryContext,
) -> Result<EffectiveTurnConfig, SurfaceSessionRecoveryError> {
    let SessionDefaults {
        metadata,
        build_state,
        allows_first_turn_build_overrides,
    } = SessionDefaults::from_session(&session)?;

    if overrides.provider.is_some() && overrides.model.is_none() {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            "provider override requires model on a session turn".to_string(),
        ));
    }
    if overrides.clear_provider_params && overrides.provider_params.is_some() {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            "clear_provider_params cannot be combined with provider_params".to_string(),
        ));
    }
    if overrides.clear_auth_binding && overrides.auth_binding.is_some() {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            "clear_auth_binding cannot be combined with auth_binding".to_string(),
        ));
    }
    if has_build_only_turn_overrides(overrides) && !allows_first_turn_build_overrides {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            BUILD_ONLY_RECOVERY_OVERRIDE_ERROR.to_string(),
        ));
    }
    if context.require_runtime_build_mode && context.runtime_build_mode.is_none() {
        return Err(SurfaceSessionRecoveryError::MissingRuntimeBuildMode(
            "runtime-backed session recovery requires canonical SessionRuntimeBindings; refusing StandaloneEphemeral fallback".to_string(),
        ));
    }

    let llm_binding = resolve_resume_llm_binding(
        metadata.provider,
        metadata.self_hosted_server_id.clone(),
        overrides.model.as_deref(),
        overrides.provider,
    );
    let resume_override_mask = ResumeOverrideMask {
        model: overrides.model.is_some(),
        provider: llm_binding.provider_overridden,
        max_tokens: overrides.max_tokens.is_some(),
        structured_output_retries: overrides.structured_output_retries.is_some(),
        provider_params: overrides.provider_params.is_some() || overrides.clear_provider_params,
        auth_binding: overrides.auth_binding.is_some() || overrides.clear_auth_binding,
        override_builtins: overrides.override_builtins.is_some(),
        override_shell: overrides.override_shell.is_some(),
        override_memory: overrides.override_memory.is_some(),
        override_workgraph: overrides.override_workgraph.is_some(),
        override_mob: overrides.override_mob.is_some(),
        override_image_generation: overrides.override_image_generation.is_some(),
        override_web_search: overrides.override_web_search.is_some(),
        preload_skills: overrides.preload_skills.is_some(),
        keep_alive: overrides.keep_alive.is_some(),
        comms_name: overrides.comms_name.is_some(),
        peer_meta: overrides.peer_meta.is_some(),
    };

    let model = overrides
        .model
        .clone()
        .unwrap_or_else(|| metadata.model.clone());
    let system_prompt = if allows_first_turn_build_overrides {
        overrides
            .system_prompt
            .clone()
            .or(build_state.system_prompt.clone())
    } else {
        None
    };
    let max_tokens = overrides.max_tokens.or(Some(metadata.max_tokens));
    let keep_alive = overrides.keep_alive.unwrap_or(metadata.keep_alive);
    let recoverable_tool_defs = overrides
        .recoverable_tool_defs
        .clone()
        .unwrap_or_else(|| build_state.recoverable_tool_defs.clone());

    let mut build = SessionBuildOptions {
        provider: llm_binding.provider,
        self_hosted_server_id: llm_binding.self_hosted_server_id,
        output_schema: overrides
            .output_schema
            .clone()
            .or(build_state.output_schema.clone()),
        structured_output_retries: overrides
            .structured_output_retries
            .unwrap_or(metadata.structured_output_retries),
        hooks_override: overrides
            .hooks_override
            .clone()
            .unwrap_or_else(|| build_state.hooks_override.clone()),
        comms_name: overrides
            .comms_name
            .clone()
            .or_else(|| metadata.comms_name.clone()),
        peer_meta: overrides
            .peer_meta
            .clone()
            .or_else(|| metadata.peer_meta.clone()),
        resume_session: Some(session),
        budget_limits: overrides
            .budget_limits
            .clone()
            .or_else(|| build_state.budget_limits.clone()),
        provider_params: if overrides.clear_provider_params {
            None
        } else {
            overrides
                .provider_params
                .clone()
                .or_else(|| metadata.provider_params.clone())
        },
        external_tools: context.external_tools,
        recoverable_tool_defs: Some(recoverable_tool_defs.clone()),
        blob_store_override: None,
        llm_client_override: context.llm_client_override,
        agent_llm_client_decorator: context.agent_llm_client_decorator,
        override_builtins: overrides
            .override_builtins
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.builtins),
        override_shell: overrides
            .override_shell
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.shell),
        override_memory: overrides
            .override_memory
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.memory),
        override_schedule: ToolCategoryOverride::Inherit,
        override_workgraph: overrides
            .override_workgraph
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.workgraph),
        override_mob: overrides
            .override_mob
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.mob),
        override_image_generation: overrides
            .override_image_generation
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.image_generation),
        override_web_search: overrides
            .override_web_search
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.web_search),
        schedule_tools: None,
        workgraph_tools: None,
        preload_skills: overrides
            .preload_skills
            .clone()
            .or_else(|| metadata.tooling.active_skills.clone()),
        realm_id: metadata.realm_id.clone().or(realm_defaults.realm_id),
        instance_id: metadata.instance_id.clone().or(realm_defaults.instance_id),
        backend: metadata.backend.clone().or(realm_defaults.backend),
        config_generation: metadata
            .config_generation
            .or(realm_defaults.config_generation),
        // Phase 3: persisted auth_binding re-entered at resume time so
        // the binding re-resolves through the same realm entry unless this
        // recovery request explicitly sets or clears it.
        auth_binding: if overrides.clear_auth_binding {
            None
        } else {
            overrides
                .auth_binding
                .clone()
                .or_else(|| metadata.auth_binding.clone())
        },
        keep_alive,
        checkpointer: context.checkpointer,
        silent_comms_intents: build_state.silent_comms_intents.clone(),
        max_inline_peer_notifications: build_state.max_inline_peer_notifications,
        app_context: overrides
            .app_context
            .clone()
            .or_else(|| build_state.app_context.clone()),
        additional_instructions: build_state.additional_instructions.clone(),
        initial_metadata_entries: std::collections::BTreeMap::new(),
        shell_env: overrides
            .shell_env
            .clone()
            .or_else(|| build_state.shell_env.clone()),
        mob_tool_authority_context: None,
        call_timeout_override: build_state.call_timeout_override,
        resume_override_mask,
        mob_tools: None,
        runtime_build_mode: context
            .runtime_build_mode
            .unwrap_or(RuntimeBuildMode::StandaloneEphemeral),
        initial_turn_metadata: None,
    };
    build.apply_persisted_mob_operator_access(
        overrides
            .override_mob
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.mob),
        build_state.mob_tool_authority_context,
    );

    Ok(EffectiveTurnConfig {
        model,
        system_prompt,
        max_tokens,
        keep_alive,
        build,
        recoverable_tool_defs,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    use crate::skills::{SkillName, SourceUuid};
    use crate::time_compat::Duration;
    use crate::{
        CallTimeoutOverride, HookEntryConfig, HookId, SessionBuildState, SessionDeferredTurnState,
        SessionMetadata, SessionTooling, ToolCategoryOverride,
    };
    use serde_json::json;

    fn skill_key(name: &str) -> SkillKey {
        SkillKey::new(
            SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("valid source uuid fixture"),
            SkillName::parse(name).expect("valid skill name fixture"),
        )
    }

    fn auth_binding(realm: &str, binding: &str) -> AuthBindingRef {
        AuthBindingRef {
            realm: crate::connection::RealmId::parse(realm).expect("valid realm fixture"),
            binding: crate::connection::BindingId::parse(binding).expect("valid binding fixture"),
            profile: None,
        }
    }

    fn sample_session() -> Session {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                schema_version: crate::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 4096,
                structured_output_retries: 3,
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: Some(json!({ "temperature": 0.1 })),
                tooling: SessionTooling {
                    builtins: ToolCategoryOverride::Disable,
                    shell: ToolCategoryOverride::Enable,
                    comms: ToolCategoryOverride::Inherit,
                    mob: ToolCategoryOverride::Inherit,
                    memory: ToolCategoryOverride::Enable,
                    workgraph: ToolCategoryOverride::Inherit,
                    image_generation: ToolCategoryOverride::Inherit,
                    web_search: ToolCategoryOverride::Inherit,
                    active_skills: Some(vec![skill_key("persisted-skill")]),
                },
                keep_alive: false,
                comms_name: Some("peer-a".to_string()),
                peer_meta: Some(
                    PeerMeta::default()
                        .with_description("persisted peer")
                        .with_label("tier", "persisted"),
                ),
                realm_id: Some("realm-a".to_string()),
                instance_id: Some("instance-a".to_string()),
                backend: Some("sqlite".to_string()),
                config_generation: Some(7),
                auth_binding: None,
            })
            .expect("session metadata");
        session
            .set_build_state(SessionBuildState {
                system_prompt: Some("persisted system prompt".to_string()),
                output_schema: Some(
                    OutputSchema::from_json_value(
                        json!({"type":"object","properties":{"ok":{"type":"boolean"}}}),
                    )
                    .expect("persisted output schema"),
                ),
                hooks_override: HookRunOverrides {
                    entries: vec![HookEntryConfig::default()],
                    disable: vec![HookId::new("disabled-hook")],
                },
                budget_limits: Some(BudgetLimits {
                    max_tokens: Some(55),
                    max_duration: Some(Duration::from_secs(9)),
                    max_tool_calls: Some(2),
                }),
                recoverable_tool_defs: vec![ToolDef {
                    name: "inline_tool".into(),
                    description: "recoverable inline tool".to_string(),
                    input_schema: json!({"type":"object"}),
                    provenance: None,
                }],
                silent_comms_intents: vec!["peer-b".to_string()],
                max_inline_peer_notifications: Some(4),
                app_context: Some(json!({ "surface": "rpc" })),
                additional_instructions: Some(vec!["be precise".to_string()]),
                shell_env: Some(HashMap::from([(
                    "MEERKAT_MODE".to_string(),
                    "test".to_string(),
                )])),
                mob_tool_authority_context: Some(
                    crate::service::MobToolAuthorityContext::new(
                        crate::service::OpaquePrincipalToken::new("persisted-authority"),
                        false,
                    )
                    .grant_manage_mob("mob-a"),
                ),
                call_timeout_override: CallTimeoutOverride::Value(Duration::from_secs(42)),
            })
            .expect("session build state");
        let mut deferred = SessionDeferredTurnState::default();
        deferred.mark_initial_turn_pending();
        session
            .set_deferred_turn_state(deferred)
            .expect("deferred turn state");
        session
    }

    #[test]
    fn resolve_effective_turn_config_plain_resume_inherits_session_defaults_before_realm() {
        let mut session = sample_session();
        let persisted_ref = auth_binding("persisted", "default");
        let mut metadata = session.session_metadata().expect("session metadata");
        metadata.auth_binding = Some(persisted_ref.clone());
        metadata.keep_alive = true;
        session
            .set_session_metadata(metadata.clone())
            .expect("updated session metadata");

        let effective = resolve_effective_turn_config(
            session,
            RealmDefaults {
                realm_id: Some("realm-fallback".to_string()),
                instance_id: Some("instance-fallback".to_string()),
                backend: Some("jsonl".to_string()),
                config_generation: Some(99),
            },
            &TurnOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("effective turn config");

        assert_eq!(effective.model, metadata.model);
        assert_eq!(effective.max_tokens, Some(metadata.max_tokens));
        assert!(effective.keep_alive);
        assert_eq!(effective.build.provider, Some(metadata.provider));
        assert_eq!(effective.build.provider_params, metadata.provider_params);
        assert_eq!(effective.build.auth_binding, Some(persisted_ref));
        assert_eq!(effective.build.override_builtins, metadata.tooling.builtins);
        assert_eq!(effective.build.override_shell, metadata.tooling.shell);
        assert_eq!(effective.build.override_memory, metadata.tooling.memory);
        assert_eq!(
            effective.build.override_mob,
            ToolCategoryOverride::Enable,
            "persisted mob authority is durable session state and rehydrates mob access"
        );
        assert!(effective.build.mob_tool_authority_context.is_some());
        assert_eq!(
            effective.build.preload_skills,
            metadata.tooling.active_skills
        );
        assert_eq!(effective.build.realm_id, metadata.realm_id);
        assert_eq!(effective.build.instance_id, metadata.instance_id);
        assert_eq!(effective.build.backend, metadata.backend);
        assert_eq!(
            effective.build.config_generation,
            metadata.config_generation
        );
        assert_eq!(
            effective.build.resume_override_mask,
            crate::service::ResumeOverrideMask::default()
        );
        assert_eq!(effective.recoverable_tool_defs.len(), 1);
        assert_eq!(effective.recoverable_tool_defs[0].name, "inline_tool");
    }

    #[test]
    fn build_recovered_session_prefers_overrides_and_preserves_persisted_build_state() {
        let override_schema = OutputSchema::from_json_value(json!({
            "type":"array",
            "items":{"type":"string"}
        }))
        .expect("override output schema");
        let recovered = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                model: Some("gpt-5.2".to_string()),
                provider: Some(Provider::OpenAI),
                provider_params: Some(json!({ "reasoning": { "effort": "medium" } })),
                max_tokens: Some(2048),
                system_prompt: Some("override system prompt".to_string()),
                output_schema: Some(override_schema.clone()),
                structured_output_retries: Some(9),
                keep_alive: Some(true),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session");

        assert_eq!(recovered.model, "gpt-5.2");
        assert_eq!(
            recovered.system_prompt.as_deref(),
            Some("override system prompt")
        );
        assert_eq!(recovered.max_tokens, Some(2048));
        assert!(recovered.keep_alive);
        assert_eq!(recovered.recoverable_tool_defs.len(), 1);
        assert_eq!(recovered.recoverable_tool_defs[0].name, "inline_tool");

        let build = recovered.build;
        assert_eq!(build.provider, Some(Provider::OpenAI));
        assert_eq!(
            build.provider_params,
            Some(json!({ "reasoning": { "effort": "medium" } }))
        );
        assert_eq!(build.structured_output_retries, 9);
        assert_eq!(build.comms_name.as_deref(), Some("peer-a"));
        assert_eq!(build.realm_id.as_deref(), Some("realm-a"));
        assert_eq!(build.instance_id.as_deref(), Some("instance-a"));
        assert_eq!(build.backend.as_deref(), Some("sqlite"));
        assert_eq!(build.config_generation, Some(7));
        assert!(build.keep_alive);
        assert_eq!(build.override_builtins, ToolCategoryOverride::Disable);
        assert_eq!(build.override_shell, ToolCategoryOverride::Enable);
        assert_eq!(build.override_mob, ToolCategoryOverride::Enable);
        assert_eq!(build.override_memory, ToolCategoryOverride::Enable);
        assert_eq!(
            build
                .mob_tool_authority_context
                .as_ref()
                .map(|authority| authority.managed_mob_scope().clone()),
            Some(std::collections::BTreeSet::from([String::from("mob-a")]))
        );
        assert_eq!(build.silent_comms_intents, vec!["peer-b".to_string()]);
        assert_eq!(build.max_inline_peer_notifications, Some(4));
        assert_eq!(build.app_context, Some(json!({ "surface": "rpc" })));
        assert_eq!(
            build.additional_instructions,
            Some(vec!["be precise".to_string()])
        );
        assert_eq!(
            build.shell_env,
            Some(HashMap::from([(
                "MEERKAT_MODE".to_string(),
                "test".to_string(),
            )]))
        );
        assert_eq!(
            build.call_timeout_override,
            CallTimeoutOverride::Value(Duration::from_secs(42))
        );
        assert_eq!(
            build
                .budget_limits
                .expect("budget limits should survive recovery")
                .max_tool_calls,
            Some(2)
        );
        assert_eq!(
            build
                .recoverable_tool_defs
                .expect("recoverable tool defs should survive recovery")[0]
                .name,
            "inline_tool"
        );
        let recovered_schema = serde_json::to_value(
            build
                .output_schema
                .expect("override output schema should be used"),
        )
        .expect("serialize recovered output schema");
        let override_schema_json =
            serde_json::to_value(override_schema).expect("serialize override schema");
        assert_eq!(recovered_schema, override_schema_json);
    }

    #[test]
    fn build_recovered_session_clear_provider_params_stays_explicit_through_resume_mask() {
        let recovered = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                clear_provider_params: true,
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session");

        assert_eq!(recovered.build.provider_params, None);
        assert!(
            recovered.build.resume_override_mask.provider_params,
            "clear provider params must prevent factory metadata rehydration"
        );
    }

    #[test]
    fn build_recovered_session_auth_binding_override_and_clear_stay_explicit() {
        let mut session = sample_session();
        let mut metadata = session.session_metadata().expect("session metadata");
        metadata.auth_binding = Some(auth_binding("persisted", "default"));
        session
            .set_session_metadata(metadata)
            .expect("updated session metadata");

        let override_ref = auth_binding("override", "default");
        let recovered = build_recovered_session(
            session.clone(),
            &SurfaceSessionRecoveryOverrides {
                auth_binding: Some(override_ref.clone()),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session with auth_binding override");

        assert_eq!(recovered.build.auth_binding, Some(override_ref));
        assert!(
            recovered.build.resume_override_mask.auth_binding,
            "set auth_binding must prevent persisted metadata overwrite"
        );

        let recovered = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides {
                clear_auth_binding: true,
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session with auth_binding clear");

        assert_eq!(recovered.build.auth_binding, None);
        assert!(
            recovered.build.resume_override_mask.auth_binding,
            "clear auth_binding must prevent factory metadata rehydration"
        );
    }

    #[test]
    fn build_recovered_session_rejects_invalid_set_and_clear_recovery_overrides() {
        let provider_error = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                provider_params: Some(json!({ "temperature": 0.2 })),
                clear_provider_params: true,
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect_err("set plus clear provider params must be rejected");
        assert!(
            provider_error.to_string().contains("clear_provider_params"),
            "unexpected error: {provider_error}"
        );

        let connection_error = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                auth_binding: Some(auth_binding("override", "default")),
                clear_auth_binding: true,
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect_err("set plus clear auth_binding must be rejected");
        assert!(
            connection_error.to_string().contains("clear_auth_binding"),
            "unexpected error: {connection_error}"
        );
    }

    #[test]
    fn build_recovered_session_can_override_metadata_and_tooling_fields() {
        let override_tools = vec![ToolDef {
            name: "fresh_tool".into(),
            description: "fresh".to_string(),
            input_schema: json!({"type":"object"}),
            provenance: None,
        }];
        let recovered = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                hooks_override: Some(HookRunOverrides::default()),
                comms_name: Some("peer-b".to_string()),
                peer_meta: Some(
                    PeerMeta::default()
                        .with_description("override peer")
                        .with_label("tier", "override"),
                ),
                budget_limits: Some(BudgetLimits {
                    max_tokens: Some(77),
                    max_duration: Some(Duration::from_secs(12)),
                    max_tool_calls: Some(5),
                }),
                override_builtins: Some(true),
                override_shell: Some(false),
                override_memory: Some(false),
                override_mob: Some(true),
                preload_skills: Some(vec![skill_key("override-skill")]),
                app_context: Some(json!({ "surface": "override" })),
                shell_env: Some(HashMap::from([(
                    "MEERKAT_MODE".to_string(),
                    "override".to_string(),
                )])),
                recoverable_tool_defs: Some(override_tools),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session");

        let build = recovered.build;
        assert_eq!(build.hooks_override, HookRunOverrides::default());
        assert_eq!(build.comms_name.as_deref(), Some("peer-b"));
        assert_eq!(
            build
                .peer_meta
                .as_ref()
                .and_then(|meta| meta.description.as_deref()),
            Some("override peer")
        );
        assert_eq!(
            build
                .budget_limits
                .as_ref()
                .and_then(|limits| limits.max_tool_calls),
            Some(5)
        );
        assert_eq!(build.override_builtins, ToolCategoryOverride::Enable);
        assert_eq!(build.override_shell, ToolCategoryOverride::Disable);
        assert_eq!(build.override_memory, ToolCategoryOverride::Disable);
        assert_eq!(build.override_mob, ToolCategoryOverride::Enable);
        assert_eq!(
            build.preload_skills,
            Some(vec![skill_key("override-skill")])
        );
        assert_eq!(build.app_context, Some(json!({ "surface": "override" })));
        assert_eq!(
            build.shell_env,
            Some(HashMap::from([(
                "MEERKAT_MODE".to_string(),
                "override".to_string(),
            )]))
        );
        let recovered_tool_defs = build
            .recoverable_tool_defs
            .expect("override recoverable tool defs should be present");
        assert_eq!(recovered_tool_defs.len(), 1);
        assert_eq!(recovered_tool_defs[0].name, "fresh_tool");
        assert_eq!(recovered.recoverable_tool_defs.len(), 1);
        assert_eq!(recovered.recoverable_tool_defs[0].name, "fresh_tool");
    }

    #[test]
    fn build_recovered_session_uses_context_fallback_for_realm_metadata() {
        let mut session = sample_session();
        let mut metadata = session.session_metadata().expect("session metadata");
        metadata.realm_id = None;
        metadata.instance_id = None;
        metadata.backend = None;
        metadata.config_generation = None;
        session
            .set_session_metadata(metadata)
            .expect("updated session metadata");

        let recovered = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext {
                realm_id: Some("realm-from-context".to_string()),
                instance_id: Some("instance-from-context".to_string()),
                backend: Some("sqlite".to_string()),
                config_generation: Some(99),
                ..Default::default()
            },
        )
        .expect("recovered session with context fallback");

        assert_eq!(
            recovered.build.realm_id.as_deref(),
            Some("realm-from-context")
        );
        assert_eq!(
            recovered.build.instance_id.as_deref(),
            Some("instance-from-context")
        );
        assert_eq!(recovered.build.backend.as_deref(), Some("sqlite"));
        assert_eq!(recovered.build.config_generation, Some(99));
    }

    #[test]
    fn build_recovered_session_rejects_provider_override_without_model() {
        let error = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                provider: Some(Provider::OpenAI),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect_err("provider-only recovery override must be rejected");

        assert!(
            matches!(error, SurfaceSessionRecoveryError::InvalidOverride(_)),
            "provider-only override should be treated as InvalidOverride"
        );
    }

    #[test]
    fn build_recovered_session_rejects_missing_runtime_build_mode_when_required() {
        let error = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext {
                require_runtime_build_mode: true,
                ..Default::default()
            },
        )
        .expect_err("runtime-backed recovery must reject silent standalone fallback");

        assert!(
            matches!(
                error,
                SurfaceSessionRecoveryError::MissingRuntimeBuildMode(_)
            ),
            "missing runtime bindings should be called out explicitly"
        );
    }

    #[test]
    fn build_recovered_session_model_override_clears_persisted_self_hosted_binding() {
        let mut session = sample_session();
        let mut metadata = session.session_metadata().expect("session metadata");
        metadata.model = "gemma-4-e2b".to_string();
        metadata.provider = Provider::SelfHosted;
        metadata.self_hosted_server_id = Some("local".to_string());
        session
            .set_session_metadata(metadata)
            .expect("updated session metadata");

        let recovered = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides {
                model: Some("gemma-4-31b".to_string()),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session with model override");

        assert_eq!(recovered.model, "gemma-4-31b");
        assert_eq!(
            recovered.build.provider, None,
            "model-only overrides should recompute provider from the new model"
        );
        assert_eq!(
            recovered.build.self_hosted_server_id, None,
            "model-only overrides must clear the persisted self-hosted server binding"
        );
        assert!(
            recovered.build.resume_override_mask.provider,
            "model-only overrides must block stale provider restoration during resume"
        );
    }

    #[test]
    fn build_recovered_session_rejects_build_only_overrides_after_first_turn_started() {
        let mut session = sample_session();
        let mut deferred = session
            .deferred_turn_state()
            .expect("deferred turn state should exist");
        assert!(
            deferred.mark_initial_turn_started(),
            "pending deferred phase should transition to consumed"
        );
        session
            .set_deferred_turn_state(deferred)
            .expect("updated deferred turn state");

        let error = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides {
                system_prompt: Some("late override".to_string()),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect_err("late build-only override should be rejected");

        assert!(
            matches!(error, SurfaceSessionRecoveryError::InvalidOverride(_)),
            "late build-only override should be treated as InvalidOverride"
        );
        assert_eq!(error.to_string(), BUILD_ONLY_RECOVERY_OVERRIDE_ERROR);
    }

    #[test]
    fn build_recovered_session_preserves_existing_system_prompt_after_first_turn_started() {
        let mut session = sample_session();
        let mut deferred = session
            .deferred_turn_state()
            .expect("deferred turn state should exist");
        assert!(
            deferred.mark_initial_turn_started(),
            "pending deferred phase should transition to consumed"
        );
        session
            .set_deferred_turn_state(deferred)
            .expect("updated deferred turn state");

        let recovered = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session without rewriting prompt");

        assert!(
            recovered.system_prompt.is_none(),
            "consumed sessions must not re-emit a create-time system_prompt override"
        );
    }

    #[test]
    fn has_materialization_overrides_ignores_live_turn_only_inputs() {
        let mut overrides = SurfaceSessionRecoveryOverrides::default();
        assert!(
            !has_materialization_overrides(&overrides),
            "empty overrides should stay on the live path"
        );

        overrides.keep_alive = Some(true);
        assert!(
            !has_materialization_overrides(&overrides),
            "keep_alive can be updated through the live path"
        );

        overrides.keep_alive = None;
        overrides.override_builtins = Some(false);
        assert!(
            has_materialization_overrides(&overrides),
            "tooling overrides require recovery/materialization"
        );
    }

    #[test]
    fn session_allows_first_turn_build_overrides_tracks_deferred_phase() {
        let mut session = sample_session();
        assert!(
            session_allows_first_turn_build_overrides(&session),
            "pending deferred phase should allow build-only overrides"
        );

        let mut deferred = session
            .deferred_turn_state()
            .expect("deferred turn state should exist");
        assert!(
            deferred.mark_initial_turn_started(),
            "pending deferred phase should transition to consumed"
        );
        session
            .set_deferred_turn_state(deferred)
            .expect("updated deferred turn state");

        assert!(
            !session_allows_first_turn_build_overrides(&session),
            "consumed deferred phase should reject build-only overrides"
        );
    }
}
