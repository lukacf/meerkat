//! Session durable-config authority — shell adapter over the canonical
//! [`SessionDocumentMachine`](crate::generated::session_document).
//!
//! Every SEMANTIC admission decision for durable session config lives in the
//! canonical machine's durable-config region (folded from the retired
//! `SessionDurableConfigAuthorityMachine` under LUC-524, P0 Dogma Invariant 1):
//!
//! - metadata-persist admission (`schema_version > 0 && model_present`),
//! - build-state-persist consistency admission (mob-tool authority context
//!   must be absent or generated-authority),
//! - build-state restore authorization (the recovery half of the same fact),
//! - system-prompt-mutation admission (`prompt_present || prompt_byte_count == 0`).
//!
//! This module performs only the MECHANICAL work the DSL cannot express: it
//! extracts typed presence/count/kind observations from the bulky
//! [`SessionMetadata`](crate::SessionMetadata) /
//! [`SessionBuildState`](crate::SessionBuildState) records and feeds them to
//! the machine, then mirrors the machine's admit/reject verdict. It NEVER
//! decides admission itself, and it passes the original typed value through
//! unchanged on admit — no fact is pre-reduced before the machine sees it.

use crate::generated::session_document::{
    self, SessionDocumentEffect, SessionDocumentError, SessionDurableProviderKind,
};
use crate::{
    CallTimeoutOverride, Provider, SessionBuildState, SessionMetadata, SessionTooling,
    ToolCategoryOverride,
};

/// Typed provenance class for a system-prompt mutation request.
///
/// This is the meerkat-core domain mirror of
/// [`session_document::SessionSystemPromptSource`]; producers construct it with
/// their typed intent and it crosses the machine seam as a typed fact (no
/// `source` string folklore). The mutation guard does not branch on it — the
/// verdict is decided from prompt presence — but pinning the class typed keeps
/// the producer's intent explicit at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSystemPromptSource {
    DirectMutation,
    ExplicitBuild,
    DefaultBuild,
    WasmDefaultBuild,
    RuntimeContextAppend,
    RuntimeSteerCleanup,
}

impl From<SessionSystemPromptSource> for session_document::SessionSystemPromptSource {
    fn from(value: SessionSystemPromptSource) -> Self {
        match value {
            SessionSystemPromptSource::DirectMutation => Self::DirectMutation,
            SessionSystemPromptSource::ExplicitBuild => Self::ExplicitBuild,
            SessionSystemPromptSource::DefaultBuild => Self::DefaultBuild,
            SessionSystemPromptSource::WasmDefaultBuild => Self::WasmDefaultBuild,
            SessionSystemPromptSource::RuntimeContextAppend => Self::RuntimeContextAppend,
            SessionSystemPromptSource::RuntimeSteerCleanup => Self::RuntimeSteerCleanup,
        }
    }
}

/// Error surfaced when the canonical machine rejects a durable-config request.
///
/// Carries the rejection message produced by the canonical
/// [`SessionDocumentMachine`](crate::generated::session_document) so callers
/// keep a stable durable-config error type while the underlying authority is
/// the canonical session-document machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDurableConfigAuthorityError {
    message: String,
}

impl std::fmt::Display for SessionDurableConfigAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessionDurableConfigAuthorityError {}

impl From<SessionDocumentError> for SessionDurableConfigAuthorityError {
    fn from(inner: SessionDocumentError) -> Self {
        Self {
            message: inner.to_string(),
        }
    }
}

/// Metadata whose persist the canonical machine has authorized.
#[derive(Debug, Clone)]
pub struct AuthorizedSessionMetadata {
    metadata: SessionMetadata,
}

impl AuthorizedSessionMetadata {
    #[must_use]
    pub fn into_metadata(self) -> SessionMetadata {
        self.metadata
    }
}

/// Build state whose persist the canonical machine has authorized.
#[derive(Debug, Clone)]
pub struct AuthorizedSessionBuildState {
    state: SessionBuildState,
}

impl AuthorizedSessionBuildState {
    #[must_use]
    pub fn into_state(self) -> SessionBuildState {
        self.state
    }
}

/// System prompt whose mutation the canonical machine has authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedSystemPrompt {
    prompt: String,
    replacing_existing: bool,
}

impl AuthorizedSystemPrompt {
    #[must_use]
    pub fn into_parts(self) -> (String, bool) {
        (self.prompt, self.replacing_existing)
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn provider_kind(provider: Provider) -> SessionDurableProviderKind {
    match provider {
        Provider::Anthropic => SessionDurableProviderKind::Anthropic,
        Provider::OpenAI => SessionDurableProviderKind::OpenAI,
        Provider::Gemini => SessionDurableProviderKind::Gemini,
        Provider::SelfHosted => SessionDurableProviderKind::SelfHosted,
        Provider::Other => SessionDurableProviderKind::Other,
    }
}

fn tool_override_kind(
    value: ToolCategoryOverride,
) -> session_document::SessionToolCategoryOverrideKind {
    match value {
        ToolCategoryOverride::Inherit => session_document::SessionToolCategoryOverrideKind::Inherit,
        ToolCategoryOverride::Enable => session_document::SessionToolCategoryOverrideKind::Enable,
        ToolCategoryOverride::Disable => session_document::SessionToolCategoryOverrideKind::Disable,
    }
}

fn call_timeout_override_kind(
    value: &CallTimeoutOverride,
) -> session_document::SessionCallTimeoutOverrideKind {
    match value {
        CallTimeoutOverride::Inherit => session_document::SessionCallTimeoutOverrideKind::Inherit,
        CallTimeoutOverride::Disabled => session_document::SessionCallTimeoutOverrideKind::Disabled,
        CallTimeoutOverride::Value(_) => session_document::SessionCallTimeoutOverrideKind::Value,
    }
}

fn active_skill_count(tooling: &SessionTooling) -> u64 {
    tooling
        .active_skills
        .as_ref()
        .map_or(0, |skills| usize_to_u64(skills.len()))
}

fn document_authority() -> session_document::SessionDocumentMachineAuthority {
    session_document::SessionDocumentMachineAuthority::new()
}

/// Drive the metadata-persist admission transition. Mirrors the machine verdict.
fn drive_metadata_persist(
    metadata: &SessionMetadata,
) -> Result<(), SessionDurableConfigAuthorityError> {
    let mut authority = document_authority();
    let effects = authority.authorize_session_metadata_persist(
        u64::from(metadata.schema_version),
        !metadata.model.trim().is_empty(),
        u64::from(metadata.max_tokens),
        u64::from(metadata.structured_output_retries),
        provider_kind(metadata.provider),
        metadata.self_hosted_server_id.is_some(),
        metadata.provider_params.is_some(),
        tool_override_kind(metadata.tooling.builtins),
        tool_override_kind(metadata.tooling.shell),
        tool_override_kind(metadata.tooling.comms),
        tool_override_kind(metadata.tooling.mob),
        tool_override_kind(metadata.tooling.memory),
        tool_override_kind(metadata.tooling.schedule),
        tool_override_kind(metadata.tooling.workgraph),
        tool_override_kind(metadata.tooling.image_generation),
        tool_override_kind(metadata.tooling.web_search),
        active_skill_count(&metadata.tooling),
        metadata.keep_alive,
        metadata.comms_name.is_some(),
        metadata.peer_meta.is_some(),
        metadata.realm_id.is_some(),
        metadata.instance_id.is_some(),
        metadata.backend.is_some(),
        metadata.config_generation.is_some(),
        metadata.auth_binding.is_some(),
    )?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SessionMetadataPersistAuthorized
        )
    })
}

/// Drive the build-state-persist admission transition.
fn drive_build_state_persist(
    state: &SessionBuildState,
) -> Result<(), SessionDurableConfigAuthorityError> {
    let mob_tool_authority_context_present = state.mob_tool_authority_context.is_some();
    let mob_tool_authority_context_generated = state
        .mob_tool_authority_context
        .as_ref()
        .is_some_and(|context| context.is_generated_authority_context());
    let mut authority = document_authority();
    let effects = authority.authorize_session_build_state_persist(
        state.system_prompt.is_some(),
        state.output_schema.is_some(),
        usize_to_u64(state.hooks_override.entries.len()),
        usize_to_u64(state.hooks_override.disable.len()),
        state.budget_limits.is_some(),
        usize_to_u64(state.recoverable_tool_defs.len()),
        usize_to_u64(state.silent_comms_intents.len()),
        state.max_inline_peer_notifications.is_some(),
        state.app_context.is_some(),
        state
            .additional_instructions
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        state
            .shell_env
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        mob_tool_authority_context_present,
        mob_tool_authority_context_generated,
        call_timeout_override_kind(&state.call_timeout_override),
    )?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SessionBuildStatePersistAuthorized
        )
    })
}

/// Drive the build-state restore authorization transition.
fn drive_build_state_restore(
    state: &SessionBuildState,
) -> Result<(), SessionDurableConfigAuthorityError> {
    let mut authority = document_authority();
    let effects = authority.restore_session_build_state(
        state.system_prompt.is_some(),
        state.output_schema.is_some(),
        usize_to_u64(state.hooks_override.entries.len()),
        usize_to_u64(state.hooks_override.disable.len()),
        state.budget_limits.is_some(),
        usize_to_u64(state.recoverable_tool_defs.len()),
        usize_to_u64(state.silent_comms_intents.len()),
        state.max_inline_peer_notifications.is_some(),
        state.app_context.is_some(),
        state
            .additional_instructions
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        state
            .shell_env
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        state.mob_tool_authority_context.is_some(),
        call_timeout_override_kind(&state.call_timeout_override),
    )?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SessionBuildStateRestoreAuthorized
        )
    })
}

/// Confirm the machine emitted the expected authorization effect.
///
/// A rejected request matched no transition and already surfaced as `Err`
/// above; this guards against an emitter that drove a different effect.
fn expect_effect(
    effects: &[SessionDocumentEffect],
    matches_expected: impl Fn(&SessionDocumentEffect) -> bool,
) -> Result<(), SessionDurableConfigAuthorityError> {
    if !effects.is_empty() && effects.iter().all(matches_expected) {
        Ok(())
    } else {
        Err(SessionDurableConfigAuthorityError {
            message:
                "generated session document authority emitted no durable-config authorization effect"
                    .to_string(),
        })
    }
}

/// Authorize a session-metadata persist through the canonical machine,
/// stamping the current schema version first.
pub fn authorize_session_metadata_persist(
    mut metadata: SessionMetadata,
) -> Result<AuthorizedSessionMetadata, SessionDurableConfigAuthorityError> {
    metadata.schema_version = crate::session_metadata_schema_version();
    drive_metadata_persist(&metadata)?;
    Ok(AuthorizedSessionMetadata { metadata })
}

/// Authorize restore of persisted session metadata through the canonical
/// machine (same admission contract as persist).
pub fn restore_session_metadata(
    metadata: SessionMetadata,
) -> Result<SessionMetadata, SessionDurableConfigAuthorityError> {
    drive_metadata_persist(&metadata)?;
    Ok(metadata)
}

/// Authorize a session-build-state persist through the canonical machine.
pub fn authorize_session_build_state_persist(
    state: SessionBuildState,
) -> Result<AuthorizedSessionBuildState, SessionDurableConfigAuthorityError> {
    drive_build_state_persist(&state)?;
    Ok(AuthorizedSessionBuildState { state })
}

/// Authorize restore of persisted session build state through the canonical
/// machine.
pub fn restore_session_build_state(
    state: SessionBuildState,
) -> Result<SessionBuildState, SessionDurableConfigAuthorityError> {
    drive_build_state_restore(&state)?;
    Ok(state)
}

/// Authorize a system-prompt mutation through the canonical machine.
pub fn authorize_system_prompt_mutation(
    prompt: String,
    source: SessionSystemPromptSource,
    replacing_existing: bool,
) -> Result<AuthorizedSystemPrompt, SessionDurableConfigAuthorityError> {
    let mut authority = document_authority();
    let effects = authority.authorize_system_prompt_mutation(
        source.into(),
        !prompt.is_empty(),
        usize_to_u64(prompt.len()),
        replacing_existing,
    )?;
    expect_effect(&effects, |effect| {
        matches!(
            effect,
            SessionDocumentEffect::SystemPromptMutationAuthorized
        )
    })?;
    Ok(AuthorizedSystemPrompt {
        prompt,
        replacing_existing,
    })
}
