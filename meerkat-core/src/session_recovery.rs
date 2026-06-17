use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::lifecycle::run_primitive::TurnMetadataOverride;
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
    pub provider_params:
        Option<TurnMetadataOverride<crate::lifecycle::run_primitive::ProviderParamsOverride>>,
    pub auth_binding: Option<TurnMetadataOverride<AuthBindingRef>>,
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
    pub override_comms: Option<bool>,
    pub override_memory: Option<bool>,
    pub override_schedule: Option<bool>,
    pub override_workgraph: Option<bool>,
    pub override_mob: Option<bool>,
    pub override_image_generation: Option<bool>,
    pub override_web_search: Option<bool>,
    pub preload_skills: Option<Vec<SkillKey>>,
    pub app_context: Option<serde_json::Value>,
    pub shell_env: Option<HashMap<String, String>>,
    pub recoverable_tool_defs: Option<Vec<ToolDef>>,
}

/// Typed session-store backend pinned by a realm.
///
/// This is the one semantic owner of the realm-pinned storage backend value
/// (the `"sqlite"`/`"jsonl"`/`"memory"` literal carried by `SessionMetadata.backend`,
/// `RealmConfig.backend_hint`, and the config envelopes). It is a pure,
/// feature-independent value type so it is reachable from `meerkat-core`;
/// `meerkat_store::RealmBackend` is the downstream, feature-gated store-layer
/// twin and projects to/from these same wire literals via `as_str`/`parse`.
///
/// Recovery exists specifically so a *recovery-environment hint* cannot silently
/// become durable session identity: the raw realm-context string is parsed into
/// this typed enum at the [`RealmDefaults::from_recovery_context`] boundary
/// (fail-closed — an unrecognized hint yields `None` rather than being ferried
/// through as an opaque string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecoveryBackendKind {
    /// JSONL append-only session store.
    Jsonl,
    /// In-memory session store.
    Memory,
    /// SQLite session store.
    Sqlite,
}

impl RecoveryBackendKind {
    /// Parse a raw realm/config backend string into the typed kind.
    ///
    /// Fail-closed: any unrecognized literal returns `None`, so a malformed
    /// recovery-environment hint is dropped at the boundary instead of becoming
    /// durable session identity.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "jsonl" => Some(Self::Jsonl),
            "memory" => Some(Self::Memory),
            "sqlite" => Some(Self::Sqlite),
            _ => None,
        }
    }

    /// The canonical wire literal for this backend kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Memory => "memory",
            Self::Sqlite => "sqlite",
        }
    }
}

/// Realm/config defaults that can participate in semantic recovery.
///
/// These values come from the active realm/config context. They only fill fields
/// that are not already durable session defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealmDefaults {
    pub realm_id: Option<crate::RealmId>,
    pub instance_id: Option<String>,
    /// Typed realm-pinned storage backend. Parsed (fail-closed) from the raw
    /// recovery-environment string at [`Self::from_recovery_context`] so a
    /// hint cannot silently become durable session identity.
    pub backend: Option<RecoveryBackendKind>,
    pub config_generation: Option<u64>,
}

impl RealmDefaults {
    #[must_use]
    pub fn from_recovery_context(context: &SurfaceSessionRecoveryContext) -> Self {
        Self {
            realm_id: context.realm_id.clone(),
            instance_id: context.instance_id.clone(),
            // Parse-at-boundary: the realm-context backend hint is a raw string
            // supplied by the host environment. It is interpreted into the typed
            // owner here (fail-closed); an unrecognized hint is dropped rather
            // than ferried through untyped into durable identity.
            backend: context
                .backend
                .as_deref()
                .and_then(RecoveryBackendKind::parse),
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
        let build_state = session.build_state().ok_or_else(|| {
            SurfaceSessionRecoveryError::MissingSessionBuildState(session.id().to_string())
        })?;
        Ok(Self {
            metadata,
            build_state,
            allows_first_turn_build_overrides: session_allows_first_turn_build_overrides(session),
        })
    }
}

#[derive(Clone)]
pub struct SurfaceSessionRecoveryContext {
    pub llm_client_override: Option<Arc<dyn Any + Send + Sync>>,
    pub agent_llm_client_decorator: Option<crate::AgentLlmClientDecorator>,
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    pub checkpointer: Option<Arc<dyn SessionCheckpointer>>,
    /// Runtime build mode supplied by the host.
    ///
    /// Required by construction: runtime-backed surfaces pass
    /// `RuntimeBuildMode::SessionOwned(bindings)` and standalone/ephemeral
    /// surfaces pass `RuntimeBuildMode::StandaloneEphemeral` explicitly. The
    /// previous `(Option<RuntimeBuildMode>, require_runtime_build_mode)` pair
    /// made "required but absent" representable and silently fell back to
    /// `StandaloneEphemeral` on the optional path; both states are now
    /// unrepresentable.
    pub runtime_build_mode: RuntimeBuildMode,
    pub realm_id: Option<crate::RealmId>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
}

impl Default for SurfaceSessionRecoveryContext {
    fn default() -> Self {
        Self {
            llm_client_override: None,
            agent_llm_client_decorator: None,
            external_tools: None,
            checkpointer: None,
            // Standalone/test surfaces own the default explicitly; runtime
            // surfaces must overwrite with `SessionOwned(bindings)`.
            runtime_build_mode: RuntimeBuildMode::StandaloneEphemeral,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        }
    }
}

/// Resolved semantic config for the next recovered turn.
///
/// This is the internal contract for:
///
/// `EffectiveTurnConfig = RealmDefaults + SessionDefaults +
/// SurfaceSessionRecoveryOverrides`
///
/// Non-semantic render/transport resources remain in
/// [`SurfaceSessionRecoveryContext`] and are copied into `build` only as host
/// plumbing.
#[derive(Debug)]
pub struct EffectiveTurnConfig {
    pub model: String,
    pub system_prompt: crate::config::SystemPromptOverride,
    pub max_tokens: Option<u32>,
    pub keep_alive: bool,
    pub build: SessionBuildOptions,
    pub recoverable_tool_defs: Vec<ToolDef>,
}

#[derive(Debug)]
pub struct RecoveredSessionBuild {
    pub model: String,
    pub system_prompt: crate::config::SystemPromptOverride,
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
            system_prompt: self.system_prompt,
            max_tokens: self.max_tokens,
            event_tx: None,
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
    #[error("persisted session {0} is missing session build state")]
    MissingSessionBuildState(String),
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
        || overrides.auth_binding.is_some()
        || has_build_only_turn_overrides(overrides)
        || overrides.hooks_override.is_some()
        || overrides.comms_name.is_some()
        || overrides.peer_meta.is_some()
        || overrides.budget_limits.is_some()
        || overrides.override_builtins.is_some()
        || overrides.override_shell.is_some()
        || overrides.override_comms.is_some()
        || overrides.override_memory.is_some()
        || overrides.override_schedule.is_some()
        || overrides.override_workgraph.is_some()
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

/// Mirror the canonical resume LLM-binding selection from the
/// [`SessionDocumentMachine`](crate::generated::session_document).
///
/// This is the binding-only entry point for surfaces (e.g. the REST resume
/// path) that do not run the full deferred-first-turn override admission: it
/// drives the same canonical transition with an `Inactive` first-turn phase and
/// no build-only overrides, so only the binding-relevant rules apply. The
/// machine still owns the `provider-requires-model` and clear/set conflict
/// verdicts; the shell mirrors the selection.
pub fn resolve_resume_llm_binding(
    stored_provider: Provider,
    stored_self_hosted_server_id: Option<String>,
    model_override: Option<&str>,
    provider_override: Option<Provider>,
) -> Result<ResumeLlmBinding, SurfaceSessionRecoveryError> {
    let overrides = SurfaceSessionRecoveryOverrides {
        model: model_override.map(str::to_string),
        provider: provider_override,
        ..Default::default()
    };
    authorize_resume_overrides(
        stored_provider,
        stored_self_hosted_server_id,
        &overrides,
        crate::generated::session_document::SessionFirstTurnPhase::Inactive,
    )
}

/// Mirror the canonical resume-override admission verdict from the
/// [`SessionDocumentMachine`](crate::generated::session_document) onto a typed
/// [`ResumeLlmBinding`].
///
/// This shell does NOT decide admission or binding: it feeds typed
/// presence/override observations and the RAW first-turn phase to the machine,
/// then mirrors the machine's accept verdict (with the LLM-binding selection
/// the machine chose) or maps the machine's typed rejection reason to the
/// existing recovery-error message. No verdict is pre-reduced before the
/// machine sees it.
fn authorize_resume_overrides(
    stored_provider: Provider,
    stored_self_hosted_server_id: Option<String>,
    overrides: &SurfaceSessionRecoveryOverrides,
    first_turn_phase: crate::generated::session_document::SessionFirstTurnPhase,
) -> Result<ResumeLlmBinding, SurfaceSessionRecoveryError> {
    use crate::generated::session_document::{
        self, ResumeOverrideRejection, ResumeProviderSelection, ResumeSelfHostedSelection,
        SessionDocumentEffect,
    };

    let mut authority = session_document::SessionDocumentMachineAuthority::new();
    let effects = authority
        .authorize_session_resume_overrides(
            overrides.provider.is_some(),
            overrides.model.is_some(),
            has_build_only_turn_overrides(overrides),
            first_turn_phase,
        )
        .map_err(|err| {
            SurfaceSessionRecoveryError::InvalidOverride(format!(
                "generated session document authority rejected resume override admission: {err}"
            ))
        })?;

    // Reject verdict: map the typed rejection reason to the existing recovery
    // error message. The verdict is the machine's; the shell only translates.
    if let Some(reason) = effects.iter().find_map(|effect| match effect {
        SessionDocumentEffect::SessionResumeOverridesRejected { reason } => Some(*reason),
        _ => None,
    }) {
        let message = match reason {
            ResumeOverrideRejection::ProviderRequiresModel => {
                "provider override requires model on a session turn".to_string()
            }
            ResumeOverrideRejection::BuildOnlyAfterFirstTurn => {
                BUILD_ONLY_RECOVERY_OVERRIDE_ERROR.to_string()
            }
        };
        return Err(SurfaceSessionRecoveryError::InvalidOverride(message));
    }

    // Accept verdict: mirror the machine's LLM-binding selection onto concrete
    // values. The shell supplies the concrete provider/server values the typed
    // selection points at; it does not re-derive the selection.
    let Some((provider_selection, self_hosted_selection, provider_overridden)) =
        effects.iter().find_map(|effect| match effect {
            SessionDocumentEffect::SessionResumeOverridesAuthorized {
                provider_selection,
                self_hosted_selection,
                provider_overridden,
            } => Some((
                *provider_selection,
                *self_hosted_selection,
                *provider_overridden,
            )),
            _ => None,
        })
    else {
        return Err(SurfaceSessionRecoveryError::InvalidOverride(
            "generated session document authority returned no resume override verdict".to_string(),
        ));
    };

    let provider = match provider_selection {
        ResumeProviderSelection::RecomputeFromModel => None,
        ResumeProviderSelection::UseOverride => overrides.provider.or(Some(stored_provider)),
        ResumeProviderSelection::UseStored => Some(stored_provider),
    };
    let self_hosted_server_id = match self_hosted_selection {
        ResumeSelfHostedSelection::Clear => None,
        ResumeSelfHostedSelection::Retain => stored_self_hosted_server_id,
    };

    Ok(ResumeLlmBinding {
        provider,
        self_hosted_server_id,
        provider_overridden,
    })
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
    overrides: &SurfaceSessionRecoveryOverrides,
    context: SurfaceSessionRecoveryContext,
) -> Result<EffectiveTurnConfig, SurfaceSessionRecoveryError> {
    let SessionDefaults {
        metadata,
        build_state,
        allows_first_turn_build_overrides,
    } = SessionDefaults::from_session(&session)?;

    // The resume-override admission verdict (provider-requires-model,
    // clear+set conflicts, build-only-after-first-turn) AND the effective
    // LLM-binding selection are owned by the canonical SessionDocumentMachine.
    // The shell feeds the RAW first-turn phase (not the already-reduced
    // overrides-allowed bool) and mirrors the verdict. A rejection surfaces as
    // a typed `InvalidOverride` here.
    let first_turn_phase: crate::generated::session_document::SessionFirstTurnPhase = session
        .deferred_turn_state()
        .map(|state| state.first_turn_phase())
        .unwrap_or_default()
        .into();
    let llm_binding = authorize_resume_overrides(
        metadata.provider,
        metadata.self_hosted_server_id.clone(),
        overrides,
        first_turn_phase,
    )?;

    let resume_override_mask = ResumeOverrideMask {
        model: overrides.model.is_some(),
        provider: llm_binding.provider_overridden,
        max_tokens: overrides.max_tokens.is_some(),
        structured_output_retries: overrides.structured_output_retries.is_some(),
        // `Some(_)` is the explicit intent (Set or Clear); `None` inherits.
        provider_params: overrides.provider_params.is_some(),
        auth_binding: overrides.auth_binding.is_some(),
        override_builtins: overrides.override_builtins.is_some(),
        override_shell: overrides.override_shell.is_some(),
        override_comms: overrides.override_comms.is_some(),
        override_memory: overrides.override_memory.is_some(),
        override_schedule: overrides.override_schedule.is_some(),
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
        match overrides.system_prompt.clone() {
            Some(prompt) => crate::config::SystemPromptOverride::Set(prompt),
            None => build_state.system_prompt.clone(),
        }
    } else {
        crate::config::SystemPromptOverride::Inherit
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
        // Caller-scoped build inputs, not durable session facts: the owning
        // surface (e.g. the mob runtime, which rebuilds from its definition)
        // re-supplies these on resume.
        custom_models: std::collections::BTreeMap::new(),
        image_generation_provider: None,
        auto_compact_threshold_override: None,
        output_schema: overrides
            .output_schema
            .clone()
            .or(build_state.output_schema.clone()),
        // Resumed sessions keep their persisted retry budget (override wins if
        // present); wrap as explicit intent so the factory seam does not fall
        // back to the live config default for an already-persisted session.
        structured_output_retries: Some(
            overrides
                .structured_output_retries
                .unwrap_or(metadata.structured_output_retries),
        ),
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
        provider_params: match &overrides.provider_params {
            Some(TurnMetadataOverride::Clear) => None,
            Some(TurnMetadataOverride::Set(value)) => Some(value.clone()),
            None => metadata.provider_params.clone(),
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
        override_comms: overrides
            .override_comms
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.comms),
        override_memory: overrides
            .override_memory
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.memory),
        override_schedule: overrides
            .override_schedule
            .map(ToolCategoryOverride::from_effective)
            .unwrap_or(metadata.tooling.schedule),
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
        // Durable `metadata.backend` wins; the realm hint only fills an absent
        // value. Both reach the build as the typed [`RecoveryBackendKind`]:
        //   - durable session truth is parsed fail-closed here (an unparseable
        //     persisted literal is dropped rather than ferried untyped), and
        //   - the realm hint is already the typed owner, having round-tripped
        //     through the same fail-closed boundary at
        //     [`RealmDefaults::from_recovery_context`].
        // A recovery-environment hint can therefore never silently become
        // durable identity: it only fills when the durable value is absent.
        backend: metadata
            .backend
            .as_deref()
            .and_then(RecoveryBackendKind::parse)
            .or(realm_defaults.backend),
        config_generation: metadata
            .config_generation
            .or(realm_defaults.config_generation),
        // Phase 3: persisted auth_binding re-entered at resume time so
        // the binding re-resolves through the same realm entry unless this
        // recovery request explicitly sets or clears it.
        auth_binding: match &overrides.auth_binding {
            Some(TurnMetadataOverride::Clear) => None,
            Some(TurnMetadataOverride::Set(value)) => Some(value.clone()),
            None => metadata.auth_binding.clone(),
        },
        // Durable mob-member identity is carried forward verbatim at resume so
        // mob ownership routing keeps recognizing the member after restart.
        mob_member_binding: metadata.mob_member_binding.clone(),
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
        initial_tool_filter: None,
        shell_env: overrides
            .shell_env
            .clone()
            .or_else(|| build_state.shell_env.clone()),
        mob_tool_authority_context: None,
        call_timeout_override: build_state.call_timeout_override,
        resume_override_mask,
        mob_tools: None,
        runtime_build_mode: context.runtime_build_mode,
        initial_turn_metadata: None,
    };
    if let Some(override_mob) = overrides.override_mob {
        build.apply_generated_create_only_mob_operator_access(
            ToolCategoryOverride::from_effective(override_mob),
        );
    } else {
        build.apply_persisted_mob_operator_access(
            metadata.tooling.mob,
            build_state.mob_tool_authority_context,
        );
    }

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
            origin: crate::connection::BindingOrigin::Configured,
        }
    }

    fn sample_session() -> Session {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                schema_version: crate::session_metadata_schema_version(),
                model: "test-anthropic-default".to_string(),
                max_tokens: 4096,
                structured_output_retries: 3,
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: Some(crate::lifecycle::run_primitive::ProviderParamsOverride {
                    temperature: Some(0.1),
                    ..Default::default()
                }),
                tooling: SessionTooling {
                    builtins: ToolCategoryOverride::Disable,
                    shell: ToolCategoryOverride::Enable,
                    comms: ToolCategoryOverride::Inherit,
                    mob: ToolCategoryOverride::Inherit,
                    memory: ToolCategoryOverride::Enable,
                    schedule: ToolCategoryOverride::Enable,
                    workgraph: ToolCategoryOverride::Enable,
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
                realm_id: Some(crate::RealmId::parse("realm-a").unwrap()),
                instance_id: Some("instance-a".to_string()),
                backend: Some("sqlite".to_string()),
                config_generation: Some(7),
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata");
        session
            .set_build_state(SessionBuildState {
                system_prompt: crate::config::SystemPromptOverride::Set(
                    "persisted system prompt".to_string(),
                ),
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
                    crate::service::MobToolAuthorityContext::generated_for_test(
                        crate::service::OpaquePrincipalToken::new("persisted-authority"),
                        false,
                        false,
                        false,
                        std::collections::BTreeSet::from(["mob-a".to_string()]),
                        std::collections::BTreeMap::new(),
                        None,
                        None,
                    ),
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
        metadata.tooling.mob = ToolCategoryOverride::Enable;
        session
            .set_session_metadata(metadata.clone())
            .expect("updated session metadata");

        let effective = resolve_effective_turn_config(
            session,
            RealmDefaults {
                realm_id: Some(crate::RealmId::parse("realm-fallback").unwrap()),
                instance_id: Some("instance-fallback".to_string()),
                backend: Some(RecoveryBackendKind::Jsonl),
                config_generation: Some(99),
            },
            &SurfaceSessionRecoveryOverrides::default(),
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
        assert_eq!(effective.build.override_comms, metadata.tooling.comms);
        assert_eq!(effective.build.override_memory, metadata.tooling.memory);
        assert_eq!(effective.build.override_schedule, metadata.tooling.schedule);
        assert_eq!(
            effective.build.override_workgraph,
            metadata.tooling.workgraph
        );
        assert_eq!(
            effective.build.override_mob,
            ToolCategoryOverride::Inherit,
            "metadata-only mob enablement must not become behavior authority"
        );
        assert!(
            effective.build.mob_tool_authority_context.is_none(),
            "projection-only mob authority must not restore behavior authority"
        );
        assert_eq!(
            effective.build.preload_skills,
            metadata.tooling.active_skills
        );
        assert_eq!(effective.build.realm_id, metadata.realm_id);
        assert_eq!(effective.build.instance_id, metadata.instance_id);
        // Durable `metadata.backend` ("sqlite") reaches the build as the typed
        // owner, parsed fail-closed from the persisted literal.
        assert_eq!(effective.build.backend, Some(RecoveryBackendKind::Sqlite));
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
                model: Some("test-openai-other".to_string()),
                provider: Some(Provider::OpenAI),
                provider_params: Some(TurnMetadataOverride::Set(
                    crate::lifecycle::run_primitive::ProviderParamsOverride {
                        reasoning: Some(crate::lifecycle::run_primitive::ReasoningMode::Emit),
                        ..Default::default()
                    },
                )),
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

        assert_eq!(recovered.model, "test-openai-other");
        assert_eq!(
            recovered.system_prompt.as_set_prompt(),
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
            Some(crate::lifecycle::run_primitive::ProviderParamsOverride {
                reasoning: Some(crate::lifecycle::run_primitive::ReasoningMode::Emit),
                ..Default::default()
            })
        );
        assert_eq!(build.structured_output_retries, Some(9));
        assert_eq!(build.comms_name.as_deref(), Some("peer-a"));
        assert_eq!(
            build.realm_id.as_ref().map(crate::RealmId::as_str),
            Some("realm-a")
        );
        assert_eq!(build.instance_id.as_deref(), Some("instance-a"));
        assert_eq!(build.backend, Some(RecoveryBackendKind::Sqlite));
        assert_eq!(build.config_generation, Some(7));
        assert!(build.keep_alive);
        assert_eq!(build.override_builtins, ToolCategoryOverride::Disable);
        assert_eq!(build.override_shell, ToolCategoryOverride::Enable);
        assert_eq!(build.override_mob, ToolCategoryOverride::Inherit);
        assert_eq!(build.override_memory, ToolCategoryOverride::Enable);
        assert!(
            build.mob_tool_authority_context.is_none(),
            "core recovery must not forward projection-only mob authority as behavior authority"
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
                provider_params: Some(TurnMetadataOverride::Clear),
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
                auth_binding: Some(TurnMetadataOverride::Set(override_ref.clone())),
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
                auth_binding: Some(TurnMetadataOverride::Clear),
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

    // NOTE: the "set + clear" fourth state is now structurally unrepresentable
    // in `SurfaceSessionRecoveryOverrides` — `provider_params`/`auth_binding`
    // are a single `Option<TurnMetadataOverride<T>>`, so a value cannot be both
    // set and cleared. The legacy-wire `clear_* + value` rejection is enforced
    // once at the serde boundary of the wire seam types (see
    // `meerkat-contracts` runtime/mob wire-override tests).

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
                override_comms: Some(false),
                override_memory: Some(false),
                override_schedule: Some(false),
                override_workgraph: Some(false),
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
        assert_eq!(build.override_comms, ToolCategoryOverride::Disable);
        assert_eq!(build.override_memory, ToolCategoryOverride::Disable);
        assert_eq!(build.override_schedule, ToolCategoryOverride::Disable);
        assert_eq!(build.override_workgraph, ToolCategoryOverride::Disable);
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
    fn build_recovered_session_comms_override_resolves_to_session_build_option() {
        // Parity with the override_shell path: a recovery request that sets
        // override_comms must surface as a typed comms tooling override on the
        // resumed turn's build options (and flag the resume mask), rather than
        // having no build/recovery authority seam at all.
        let recovered = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides {
                override_comms: Some(false),
                ..Default::default()
            },
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session with comms override");

        assert_eq!(
            recovered.build.override_comms,
            ToolCategoryOverride::Disable,
            "override_comms=Some(false) must disable comms tooling on the resumed turn"
        );
        assert!(
            recovered.build.resume_override_mask.override_comms,
            "explicit comms override must set the resume mask bit so persisted comms tooling is not rehydrated over it"
        );

        // Absent override inherits the persisted comms tooling (Inherit in the
        // sample session).
        let inherited = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("recovered session without comms override");
        assert_eq!(
            inherited.build.override_comms,
            ToolCategoryOverride::Inherit,
            "absent comms override inherits persisted SessionTooling.comms"
        );
        assert!(
            !inherited.build.resume_override_mask.override_comms,
            "no explicit comms override must leave the resume mask bit clear"
        );
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
                realm_id: Some(crate::RealmId::parse("realm-from-context").unwrap()),
                instance_id: Some("instance-from-context".to_string()),
                backend: Some("sqlite".to_string()),
                config_generation: Some(99),
                ..Default::default()
            },
        )
        .expect("recovered session with context fallback");

        assert_eq!(
            recovered
                .build
                .realm_id
                .as_ref()
                .map(crate::RealmId::as_str),
            Some("realm-from-context")
        );
        assert_eq!(
            recovered.build.instance_id.as_deref(),
            Some("instance-from-context")
        );
        assert_eq!(
            recovered.build.backend,
            Some(RecoveryBackendKind::Sqlite),
            "a valid recovery-environment backend hint fills the absent durable value as the typed owner"
        );
        assert_eq!(recovered.build.config_generation, Some(99));
    }

    #[test]
    fn recovery_backend_kind_parses_known_literals_and_round_trips() {
        assert_eq!(
            RecoveryBackendKind::parse("sqlite"),
            Some(RecoveryBackendKind::Sqlite)
        );
        assert_eq!(
            RecoveryBackendKind::parse("jsonl"),
            Some(RecoveryBackendKind::Jsonl)
        );
        assert_eq!(
            RecoveryBackendKind::parse("memory"),
            Some(RecoveryBackendKind::Memory)
        );
        assert_eq!(RecoveryBackendKind::Sqlite.as_str(), "sqlite");
        assert_eq!(RecoveryBackendKind::Jsonl.as_str(), "jsonl");
        assert_eq!(RecoveryBackendKind::Memory.as_str(), "memory");
        for kind in [
            RecoveryBackendKind::Sqlite,
            RecoveryBackendKind::Jsonl,
            RecoveryBackendKind::Memory,
        ] {
            assert_eq!(RecoveryBackendKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn realm_defaults_parses_valid_backend_hint_into_typed_owner() {
        let realm_defaults = RealmDefaults::from_recovery_context(&SurfaceSessionRecoveryContext {
            backend: Some("sqlite".to_string()),
            ..Default::default()
        });

        assert_eq!(
            realm_defaults.backend,
            Some(RecoveryBackendKind::Sqlite),
            "a valid recovery-environment backend hint must parse into the typed owner"
        );
    }

    #[test]
    fn realm_defaults_drops_malformed_backend_hint_fail_closed() {
        let realm_defaults = RealmDefaults::from_recovery_context(&SurfaceSessionRecoveryContext {
            backend: Some("postgres".to_string()),
            ..Default::default()
        });

        assert_eq!(
            realm_defaults.backend, None,
            "an unrecognized backend hint must be dropped at the parse boundary, not ferried through as an opaque string"
        );
    }

    #[test]
    fn build_recovered_session_does_not_launder_malformed_backend_hint_into_durable_identity() {
        // A recovery-environment backend hint that does not name a known store
        // must never reach the durable session-build backend identity. With no
        // persisted `metadata.backend`, the malformed hint is dropped at the
        // typed boundary and the recovered build carries no backend.
        let mut session = sample_session();
        let mut metadata = session.session_metadata().expect("session metadata");
        metadata.backend = None;
        session
            .set_session_metadata(metadata)
            .expect("updated session metadata");

        let recovered = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext {
                backend: Some("not-a-real-backend".to_string()),
                ..Default::default()
            },
        )
        .expect("recovered session with malformed backend hint");

        assert_eq!(
            recovered.build.backend, None,
            "a malformed recovery-environment hint must not become durable session identity"
        );
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

    /// Row: recovery runtime mode is required by construction — the context
    /// carries a non-optional `RuntimeBuildMode`, so "required but absent"
    /// and "optional silent standalone fallback" are unrepresentable. The
    /// explicit default is `StandaloneEphemeral`, and the recovered build
    /// carries exactly the mode the caller supplied.
    #[test]
    fn build_recovered_session_carries_explicit_runtime_build_mode() {
        let recovered = build_recovered_session(
            sample_session(),
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("standalone recovery should succeed");

        assert!(
            matches!(
                recovered.build.runtime_build_mode,
                RuntimeBuildMode::StandaloneEphemeral
            ),
            "explicit standalone mode must flow through unchanged"
        );
    }

    #[test]
    fn build_recovered_session_rejects_missing_build_state() {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                schema_version: crate::session_metadata_schema_version(),
                model: "test-anthropic-default".to_string(),
                max_tokens: 4096,
                structured_output_retries: 3,
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("session metadata");

        let error = build_recovered_session(
            session,
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect_err("recovery must fail closed without durable build state");

        assert!(
            matches!(
                error,
                SurfaceSessionRecoveryError::MissingSessionBuildState(_)
            ),
            "missing build state should be reported explicitly"
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
            recovered.system_prompt.is_inherit(),
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

        overrides.override_builtins = None;
        overrides.override_schedule = Some(true);
        assert!(
            has_materialization_overrides(&overrides),
            "schedule tooling overrides require recovery/materialization"
        );

        overrides.override_schedule = None;
        overrides.override_workgraph = Some(true);
        assert!(
            has_materialization_overrides(&overrides),
            "WorkGraph tooling overrides require recovery/materialization"
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
