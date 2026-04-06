use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::runtime_epoch::RuntimeBuildMode;
use crate::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, ResumeOverrideMask,
    SessionBuildOptions,
};
use crate::{
    AgentToolDispatcher, BudgetLimits, ContentInput, HookRunOverrides, OutputSchema, PeerMeta,
    Provider, Session, SessionDeferredTurnState, ToolDef, skills::SkillId,
};

pub const BUILD_ONLY_RECOVERY_OVERRIDE_ERROR: &str = "Cannot override max_tokens, system_prompt, output_schema, or structured_output_retries after the deferred session's first turn has started";

#[derive(Debug, Clone, Default)]
pub struct SurfaceSessionRecoveryOverrides {
    pub model: Option<String>,
    pub provider: Option<Provider>,
    pub provider_params: Option<serde_json::Value>,
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
    pub override_mob: Option<bool>,
    pub preload_skills: Option<Vec<SkillId>>,
    pub app_context: Option<serde_json::Value>,
    pub shell_env: Option<HashMap<String, String>>,
    pub recoverable_tool_defs: Option<Vec<ToolDef>>,
}

#[derive(Clone, Default)]
pub struct SurfaceSessionRecoveryContext {
    pub llm_client_override: Option<Arc<dyn Any + Send + Sync>>,
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    pub runtime_build_mode: Option<RuntimeBuildMode>,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
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

#[derive(Debug, thiserror::Error)]
pub enum SurfaceSessionRecoveryError {
    #[error("persisted session {0} is missing session metadata")]
    MissingSessionMetadata(String),
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
        || has_build_only_turn_overrides(overrides)
        || overrides.hooks_override.is_some()
        || overrides.comms_name.is_some()
        || overrides.peer_meta.is_some()
        || overrides.budget_limits.is_some()
        || overrides.override_builtins.is_some()
        || overrides.override_shell.is_some()
        || overrides.override_memory.is_some()
        || overrides.override_mob.is_some()
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

pub fn build_recovered_session(
    session: Session,
    overrides: &SurfaceSessionRecoveryOverrides,
    context: SurfaceSessionRecoveryContext,
) -> Result<RecoveredSessionBuild, SurfaceSessionRecoveryError> {
    let metadata = session.session_metadata().ok_or_else(|| {
        SurfaceSessionRecoveryError::MissingSessionMetadata(session.id().to_string())
    })?;
    if overrides.provider.is_some() && overrides.model.is_none() {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            "provider override requires model on a session turn".to_string(),
        ));
    }
    if has_build_only_turn_overrides(overrides)
        && !session_allows_first_turn_build_overrides(&session)
    {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            BUILD_ONLY_RECOVERY_OVERRIDE_ERROR.to_string(),
        ));
    }

    let build_state = session.build_state().unwrap_or_default();
    let resume_override_mask = ResumeOverrideMask {
        model: overrides.model.is_some(),
        provider: overrides.provider.is_some(),
        max_tokens: overrides.max_tokens.is_some(),
        structured_output_retries: overrides.structured_output_retries.is_some(),
        provider_params: overrides.provider_params.is_some(),
        override_builtins: overrides.override_builtins.is_some(),
        override_shell: overrides.override_shell.is_some(),
        override_memory: overrides.override_memory.is_some(),
        override_mob: overrides.override_mob.is_some(),
        preload_skills: overrides.preload_skills.is_some(),
        keep_alive: overrides.keep_alive.is_some(),
        comms_name: overrides.comms_name.is_some(),
        peer_meta: overrides.peer_meta.is_some(),
    };

    let model = overrides
        .model
        .clone()
        .unwrap_or_else(|| metadata.model.clone());
    let system_prompt = if session_allows_first_turn_build_overrides(&session) {
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
        provider: Some(overrides.provider.unwrap_or(metadata.provider)),
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
        provider_params: overrides
            .provider_params
            .clone()
            .or_else(|| metadata.provider_params.clone()),
        external_tools: context.external_tools,
        recoverable_tool_defs: Some(recoverable_tool_defs.clone()),
        blob_store_override: None,
        llm_client_override: context.llm_client_override,
        override_builtins: overrides
            .override_builtins
            .or_else(|| metadata.tooling.builtins.to_override()),
        override_shell: overrides
            .override_shell
            .or_else(|| metadata.tooling.shell.to_override()),
        override_memory: overrides
            .override_memory
            .or_else(|| metadata.tooling.memory.to_override()),
        override_mob: overrides
            .override_mob
            .or_else(|| metadata.tooling.mob.to_override()),
        preload_skills: overrides
            .preload_skills
            .clone()
            .or_else(|| metadata.tooling.active_skills.clone()),
        realm_id: metadata.realm_id.clone().or(context.realm_id),
        instance_id: metadata.instance_id.clone().or(context.instance_id),
        backend: metadata.backend.clone().or(context.backend),
        config_generation: metadata.config_generation.or(context.config_generation),
        keep_alive,
        checkpointer: None,
        silent_comms_intents: build_state.silent_comms_intents.clone(),
        max_inline_peer_notifications: build_state.max_inline_peer_notifications,
        app_context: overrides
            .app_context
            .clone()
            .or_else(|| build_state.app_context.clone()),
        additional_instructions: build_state.additional_instructions.clone(),
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
    };
    build.apply_persisted_mob_operator_access(
        overrides
            .override_mob
            .or_else(|| metadata.tooling.mob.to_override()),
        build_state.mob_tool_authority_context,
    );

    Ok(RecoveredSessionBuild {
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

    use crate::time_compat::Duration;
    use crate::{
        CallTimeoutOverride, HookEntryConfig, HookId, SessionBuildState, SessionDeferredTurnState,
        SessionMetadata, SessionTooling, ToolCategoryOverride,
    };
    use serde_json::json;

    fn sample_session() -> Session {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 4096,
                structured_output_retries: 3,
                provider: Provider::Anthropic,
                provider_params: Some(json!({ "temperature": 0.1 })),
                tooling: SessionTooling {
                    builtins: ToolCategoryOverride::Disable,
                    shell: ToolCategoryOverride::Enable,
                    comms: ToolCategoryOverride::Inherit,
                    mob: ToolCategoryOverride::Inherit,
                    memory: ToolCategoryOverride::Enable,
                    active_skills: Some(vec![SkillId("persisted/skill".to_string())]),
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
                    name: "inline_tool".to_string(),
                    description: "recoverable inline tool".to_string(),
                    input_schema: json!({"type":"object"}),
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
        assert_eq!(build.override_builtins, Some(false));
        assert_eq!(build.override_shell, Some(true));
        assert_eq!(build.override_mob, Some(true));
        assert_eq!(build.override_memory, Some(true));
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
    fn build_recovered_session_can_override_metadata_and_tooling_fields() {
        let override_tools = vec![ToolDef {
            name: "fresh_tool".to_string(),
            description: "fresh".to_string(),
            input_schema: json!({"type":"object"}),
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
                preload_skills: Some(vec![SkillId("override/skill".to_string())]),
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
        assert_eq!(build.override_builtins, Some(true));
        assert_eq!(build.override_shell, Some(false));
        assert_eq!(build.override_memory, Some(false));
        assert_eq!(build.override_mob, Some(true));
        assert_eq!(
            build.preload_skills,
            Some(vec![SkillId("override/skill".to_string())])
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
                backend: Some("redb".to_string()),
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
        assert_eq!(recovered.build.backend.as_deref(), Some("redb"));
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
