//! `session/*` method handlers.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat::AgentBuildConfig;
use meerkat_contracts::SkillsParams;
// RPC-surface request/response types now live in meerkat-contracts (single
// source of truth, schema-emitted); the handlers consume them directly and
// re-export them so existing `handlers::session::*` paths keep resolving.
pub use meerkat_contracts::wire::{
    ArchiveSessionParams, DeferredCreateResult, InjectSystemContextParams,
    InjectSystemContextResult, ListSessionsParams, ListSessionsResult, ReadSessionHistoryParams,
    ReadSessionParams,
};
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::SessionQuery;
use meerkat_core::skills::{SkillKey, SkillRef};
use meerkat_core::{
    BudgetLimits, ContentInput, HookRunOverrides, OutputSchema, Provider, ToolCategoryOverride,
    ToolFilter,
};
use meerkat_runtime::SessionServiceRuntimeExt;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use super::skills::reject_retired_skill_references;
use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::{RpcId, RpcResponse, bounded_collection_limit, bounded_collection_offset};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;
use meerkat::surface::{RequestContext, request_action};

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

fn parse_provider_param(provider: &str) -> Result<Provider, String> {
    Provider::parse_strict(provider).ok_or_else(|| {
        format!(
            "unknown provider '{provider}' (expected anthropic, openai, gemini, or self_hosted)"
        )
    })
}

fn resolve_rpc_create_session_model(
    config: &meerkat_core::Config,
    model: Option<String>,
    provider: Option<Provider>,
    auth_binding: Option<meerkat_core::AuthBindingRef>,
) -> Result<meerkat::CreateSessionModelResolution, meerkat::CreateSessionModelResolutionError> {
    meerkat::resolve_create_session_model(
        config,
        meerkat::CreateSessionModelResolutionRequest {
            model,
            provider,
            auth_binding,
        },
    )
}

fn create_session_model_resolution_error_code(
    error: &meerkat::CreateSessionModelResolutionError,
) -> i32 {
    if error.is_configuration_fault() {
        error::INTERNAL_ERROR
    } else {
        error::INVALID_PARAMS
    }
}

/// Controls whether the first turn runs immediately on session creation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitialTurn {
    /// Run the first turn immediately (default).
    #[default]
    RunImmediately,
    /// Create the session but defer the first turn.
    Deferred,
}

/// Parameters for `session/create`.
///
/// Mirrors the fields of [`AgentBuildConfig`] that are relevant for session
/// creation via the JSON-RPC surface. Optional fields default to `None`/`false`
/// so callers only need to provide `prompt`.
#[derive(Debug, Deserialize)]
pub struct CreateSessionParams {
    pub prompt: ContentInput,
    /// Host-attached injected context for the first turn. Each entry
    /// materializes as a separate typed injected-context user-channel
    /// message immediately before the first turn's user message, in order.
    /// Injected context is excluded from semantic-memory indexing.
    #[serde(default)]
    pub injected_context: Option<Vec<ContentInput>>,
    /// Controls whether the first turn runs immediately or is deferred.
    /// Defaults to `run_immediately` when absent.
    #[serde(default)]
    pub initial_turn: Option<InitialTurn>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: meerkat::SystemPromptOverride,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation (default: 2).
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable built-in tools. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Enable shell tool. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_shell: Option<bool>,
    /// Keep the agent alive after the initial turn (enables comms drain loop).
    #[serde(default)]
    pub keep_alive: bool,
    /// Agent name for comms (required when keep_alive is true).
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (description, labels).
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Enable semantic memory. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable schedule tools. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_schedule: Option<bool>,
    /// Enable mob tools. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Enable Meerkat-owned fallback web search. Omit to keep hidden.
    #[serde(default)]
    pub enable_web_search: Option<bool>,
    /// Optional session-local tool visibility filter.
    #[serde(default)]
    pub tool_filter: Option<ToolFilter>,
    /// Enable WorkGraph tools. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_workgraph: Option<bool>,
    /// Explicit budget limits for this session.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimits>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    /// Override the realm-scoped auth binding for this session.
    #[serde(default)]
    pub auth_binding: Option<meerkat_contracts::wire::WireAuthBindingRef>,
    /// Structured skill keys to preload into the system prompt.
    #[serde(default)]
    pub preload_skills: Option<Vec<SkillKey>>,
    /// Skill IDs to resolve and inject for the first turn.
    #[serde(default)]
    pub skill_refs: Option<Vec<SkillRef>>,
    /// Retired legacy string refs. Kept only to return a typed ingress error
    /// when old clients send the field.
    #[serde(default, deserialize_with = "reject_retired_skill_references")]
    pub skill_references: Option<Vec<String>>,
    /// Key-value labels attached to the session for filtering and metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Additional instruction sections appended to the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Opaque application context passed to custom session builders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    /// Set by the application — never visible to the LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// External tool definitions provided by the client (callback tools).
    /// These are merged with globally registered tools at build time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_tools: Option<Vec<meerkat_core::ToolDef>>,
}

fn canonical_skill_ids(
    _runtime: &SessionRuntime,
    skill_refs: Option<Vec<SkillRef>>,
) -> Result<Option<Vec<SkillKey>>, meerkat_core::skills::SkillError> {
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
    };
    Ok(params.canonical_skill_keys())
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result for `session/create` — uses canonical wire type from contracts.
pub type CreateSessionResult = meerkat_contracts::WireRunResult;

/// Result for `session/read` — uses canonical wire type from contracts.
pub type ReadSessionResult = meerkat_contracts::WireSessionInfo;

/// Result for `session/history` — uses canonical wire type from contracts.
pub type ReadSessionHistoryResult = meerkat_contracts::WireSessionHistory;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `session/create`.
pub async fn handle_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let params: CreateSessionParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    create_session_with_params(
        id,
        params,
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}

/// Typed `session/create` entrypoint shared with identity-native callers
/// (`help/ask`). Takes a fully-formed [`CreateSessionParams`] so callers
/// route typed params without re-serializing through a hand-shaped JSON
/// payload (K17: handlers speak typed params/results only).
pub async fn create_session_with_params(
    id: Option<RpcId>,
    params: CreateSessionParams,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    if let Err(err) = meerkat::surface::validate_public_peer_meta(params.peer_meta.as_ref()) {
        return RpcResponse::error(id, error::INVALID_PARAMS, err);
    }
    if let Err(err) = meerkat::surface::validate_public_surface_metadata(
        params.labels.as_ref(),
        params.app_context.as_ref(),
    ) {
        return RpcResponse::error(id, error::INVALID_PARAMS, err);
    }
    let model_was_explicit = params.model.is_some();
    let provider_was_explicit = params.provider.is_some();
    let auth_binding_was_explicit = params.auth_binding.is_some();
    let provider = match params
        .provider
        .as_deref()
        .map(parse_provider_param)
        .transpose()
    {
        Ok(provider) => provider,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let effective_config = match runtime.effective_config().await {
        Ok(config) => config,
        Err(err) => return RpcResponse::error(id, err.code, err.message),
    };
    // Reconstruct server-owned binding provenance before the shared resolver;
    // clients carry only realm/binding/profile atoms.
    let auth_binding = params.auth_binding.clone().map(Into::into);
    let model_resolution = match resolve_rpc_create_session_model(
        &effective_config,
        params.model.clone(),
        provider,
        auth_binding,
    ) {
        Ok(resolution) => resolution,
        Err(err) => {
            return RpcResponse::error(
                id,
                create_session_model_resolution_error_code(&err),
                err.to_string(),
            );
        }
    };
    let model_name = model_resolution.model;

    // Parse output schema if provided
    let output_schema = match params.output_schema {
        Some(schema) => match OutputSchema::from_json_value(schema) {
            Ok(os) => Some(os),
            Err(e) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid output_schema: {e}"),
                );
            }
        },
        None => None,
    };

    let mut build_config = AgentBuildConfig::new(model_name);
    build_config.provider = Some(model_resolution.provider);
    build_config.resume_override_mask.model = model_was_explicit;
    build_config.resume_override_mask.provider = provider_was_explicit;
    build_config.max_tokens = params.max_tokens;
    build_config.system_prompt = params.system_prompt;
    build_config.output_schema = output_schema;
    build_config.structured_output_retries = params.structured_output_retries;
    build_config.hooks_override = params.hooks_override.unwrap_or_default();
    build_config.override_builtins = ToolCategoryOverride::from_override(params.enable_builtins);
    build_config.override_shell = ToolCategoryOverride::from_override(params.enable_shell);
    build_config.keep_alive = params.keep_alive;
    build_config.comms_name = params.comms_name;
    build_config.peer_meta = params.peer_meta;
    build_config.override_memory = ToolCategoryOverride::from_override(params.enable_memory);
    build_config.override_schedule = ToolCategoryOverride::from_override(params.enable_schedule);
    build_config.override_workgraph = ToolCategoryOverride::from_override(params.enable_workgraph);
    build_config.apply_generated_create_only_mob_operator_access(
        ToolCategoryOverride::from_override(params.enable_mob),
    );
    build_config.override_web_search =
        ToolCategoryOverride::from_override(params.enable_web_search);
    if let Some(tool_filter) = params.tool_filter {
        // The typed tool filter is carried directly now (no string side
        // channel that could fail to serialize), so this is infallible.
        build_config.set_initial_tool_filter(tool_filter);
    }
    // Mob tools factory — injected via FactoryAgentBuilder.default_mob_tools or
    // AgentFactory.mob_tools. No per-handler wiring needed; the factory resolves
    // it at build time.
    build_config.budget_limits = params.budget_limits;
    build_config.provider_params = params.provider_params;
    build_config.resume_override_mask.auth_binding = auth_binding_was_explicit;
    build_config.auth_binding = model_resolution.auth_binding;
    build_config.additional_instructions = params.additional_instructions;
    build_config.app_context = params.app_context;
    build_config.shell_env = params.shell_env;
    build_config.preload_skills = params.preload_skills;

    // Wire callback tools backed by the live registered_tools list.
    // Tools added later via tools/register are picked up dynamically at each
    // turn boundary (via poll_external_updates). Per-session inline tools
    // (from params.external_tools) are held separately inside the dispatcher
    // and take precedence on name collision with globals.
    {
        let inline_tools: Vec<meerkat_core::ToolDef> = params
            .external_tools
            .unwrap_or_default()
            .into_iter()
            .map(|t| meerkat_core::ToolDef {
                provenance: Some(meerkat_core::types::ToolProvenance {
                    kind: meerkat_core::types::ToolSourceKind::Callback,
                    source_id: "callback".into(),
                }),
                ..t
            })
            .collect();
        if let Some(tx) = runtime.callback_request_tx() {
            let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> =
                Arc::new(crate::callback_dispatcher::CallbackToolDispatcher::new(
                    runtime.registered_tools(),
                    tx,
                    runtime.callback_id_counter(),
                    inline_tools,
                ));
            build_config.external_tools = Some(dispatcher);
        }
    }

    // Validate and canonicalize skill refs before creating a pending session.
    // This prevents invalid requests from consuming session slots.
    let skill_refs = match canonical_skill_ids(&runtime, params.skill_refs) {
        Ok(r) => r,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid skill_refs: {e}"),
            );
        }
    };

    // Create the session. When deferred, stash the prompt (and any injected
    // context) for the first turn; both materialize at promotion.
    let injected_context = params.injected_context.unwrap_or_default();
    let (deferred_prompt, deferred_injected_context, injected_context) =
        if params.initial_turn == Some(InitialTurn::Deferred) {
            (Some(params.prompt.clone()), injected_context, Vec::new())
        } else {
            (None, Vec::new(), injected_context)
        };
    let session_id = match runtime
        .create_session(
            build_config,
            params.labels,
            deferred_prompt,
            deferred_injected_context,
        )
        .await
    {
        Ok(sid) => sid,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

    // Immediate creates need a live runtime loop before the first turn starts.
    // Deferred creates register on-demand through the session/* router entry
    // points; eagerly attaching here is redundant and can recurse through the
    // runtime control path before the pending session has ever been exercised.
    if params.initial_turn != Some(InitialTurn::Deferred) {
        let executor = Box::new(crate::session_executor::SessionRuntimeExecutor::new(
            runtime.clone(),
            session_id.clone(),
        ));
        if let Err(error) = runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await
        {
            if let Err(cleanup_error) = runtime.archive_session(&session_id).await {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!(
                        "runtime executor registration failed: {error}; staged session cleanup failed: {}",
                        cleanup_error.message
                    ),
                );
            }
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("runtime executor registration failed: {error}"),
            );
        }
    }

    if let Some(context) = request_context.as_ref() {
        let runtime_adapter_for_cancel = Arc::clone(runtime_adapter);
        let session_id_for_cancel = session_id.clone();
        let install = context
            .install_cancel_action_or_cancelled(request_action(move || {
                let runtime_adapter = Arc::clone(&runtime_adapter_for_cancel);
                let session_id = session_id_for_cancel.clone();
                async move {
                    let _ = runtime_adapter
                        .hard_cancel_current_run(&session_id, "RPC request cancelled")
                        .await;
                }
            }))
            .await;

        let runtime_for_cleanup = Arc::clone(&runtime);
        let session_id_for_cleanup = session_id.clone();
        context.set_unpublished_cleanup(request_action(move || {
            let runtime = Arc::clone(&runtime_for_cleanup);
            let session_id = session_id_for_cleanup.clone();
            async move {
                if let Err(error) = runtime.archive_session(&session_id).await {
                    tracing::error!(
                        session_id = %session_id,
                        error = %error.message,
                        "RPC unpublished create archive failed; leaving runtime state registered"
                    );
                }
            }
        }));
        if install == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            if let Err(error) = runtime.archive_session(&session_id).await {
                return RpcResponse::error(
                    id,
                    error.code,
                    format!(
                        "request cancelled before start but archive cleanup failed: {}",
                        error.message
                    ),
                );
            }
            return RpcResponse::error(
                id,
                error::REQUEST_CANCELLED,
                "request cancelled before start",
            );
        }
    }

    // Deferred mode: return session ID without running a turn.
    if params.initial_turn == Some(InitialTurn::Deferred) {
        let result = DeferredCreateResult {
            session_id: session_id.to_string(),
            session_ref: runtime
                .realm_id()
                .map(|realm| meerkat_contracts::format_session_ref(&realm, &session_id)),
        };
        return RpcResponse::success(id, result);
    }

    // Set up MCP lifecycle event forwarding. Agent execution events flow
    // through SessionRuntimeExecutor's own channel, not this one.
    let (mcp_event_tx, mut mcp_event_rx) =
        mpsc::channel::<EventEnvelope<AgentEvent>>(NOTIFICATION_CHANNEL_CAPACITY);
    let sink = notification_sink.clone();
    let sid_clone = session_id.clone();
    tokio::spawn(async move {
        while let Some(event) = mcp_event_rx.recv().await {
            sink.emit_event(&sid_clone, &event).await;
        }
    });

    // Start the initial turn — route through runtime for V9 consistency
    let result = if params.keep_alive {
        let runtime_for_turn = Arc::clone(&runtime);
        let sid_for_turn = session_id.clone();
        let event_tx_for_turn = mcp_event_tx.clone();
        let prompt_for_turn = params.prompt.clone();
        let injected_context_for_turn = injected_context.clone();
        let skill_refs_for_turn = skill_refs.clone();
        tokio::spawn(async move {
            if let Err(rpc_err) = runtime_for_turn
                .start_turn_via_runtime(
                    &sid_for_turn,
                    prompt_for_turn,
                    injected_context_for_turn,
                    event_tx_for_turn,
                    skill_refs_for_turn,
                    None,
                    None,
                    None,
                )
                .await
            {
                tracing::error!(
                    session_id = %sid_for_turn,
                    error = %rpc_err.code,
                    "Host-mode session start failed: {}",
                    rpc_err.message
                );
            }
        });

        if !await_comms_runtime_ready(&runtime, &session_id).await {
            tracing::warn!(
                session_id = %session_id,
                "Host-mode session started without comms runtime before create response timeout"
            );
        }

        meerkat_core::RunResult {
            text: String::new(),
            session_id: session_id.clone(),
            usage: Default::default(),
            turns: 0,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        }
    } else {
        match runtime
            .start_turn_via_runtime(
                &session_id,
                params.prompt,
                injected_context,
                mcp_event_tx,
                skill_refs,
                None,
                None,
                None,
            )
            .await
        {
            Ok(r) => r,
            Err(rpc_err) => {
                return RpcResponse::error(id, rpc_err.code, rpc_err.message);
            }
        }
    };

    let mut response: CreateSessionResult = result.into();
    response.session_ref = runtime
        .realm_id()
        .map(|realm| meerkat_contracts::format_session_ref(&realm, &response.session_id));
    RpcResponse::success(id, response)
}

#[cfg(feature = "comms")]
async fn await_comms_runtime_ready(
    runtime: &SessionRuntime,
    session_id: &meerkat_core::SessionId,
) -> bool {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(250);
    while tokio::time::Instant::now() < deadline {
        if runtime.comms_runtime(session_id).await.is_some() {
            return true;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    false
}

#[cfg(not(feature = "comms"))]
async fn await_comms_runtime_ready(
    _runtime: &SessionRuntime,
    _session_id: &meerkat_core::SessionId,
) -> bool {
    true
}

/// Handle `session/list`.
pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    // All fields are optional, so missing params is fine.
    let list_params: ListSessionsParams = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(p) => p,
            Err(e) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                );
            }
        },
        None => ListSessionsParams::default(),
    };

    let limit = match bounded_collection_limit(list_params.limit) {
        Ok(limit) => limit,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let offset = match bounded_collection_offset(list_params.offset) {
        Ok(offset) => offset,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let query = SessionQuery {
        labels: list_params.labels,
        limit: Some(limit),
        offset: Some(offset),
    };

    let summaries = match runtime.list_sessions_rich(query).await {
        Ok(s) => s,
        Err(err) => return RpcResponse::error(id, err.code, err.message),
    };
    let sessions: Vec<meerkat_contracts::WireSessionSummary> = summaries
        .into_iter()
        .map(|mut ws| {
            ws.session_ref = runtime
                .realm_id()
                .map(|realm| meerkat_contracts::format_session_ref(&realm, &ws.session_id));
            ws
        })
        .collect();

    let result = ListSessionsResult { sessions };
    RpcResponse::success(id, result)
}

/// Handle `session/read`.
pub async fn handle_read(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: ReadSessionParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    match runtime.read_session_rich(&session_id).await {
        // Row #98: a store fault must NOT be reported as SESSION_NOT_FOUND.
        Ok(Some(mut info)) => {
            info.session_ref = runtime
                .realm_id()
                .map(|realm| meerkat_contracts::format_session_ref(&realm, &info.session_id));
            RpcResponse::success(id, info)
        }
        Ok(None) => RpcResponse::error(
            id,
            error::SESSION_NOT_FOUND,
            format!("Session not found: {session_id}"),
        ),
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::items_after_test_module,
    clippy::unwrap_used
)]
mod tests {
    use super::{
        CreateSessionResult, create_session_model_resolution_error_code,
        resolve_rpc_create_session_model,
    };
    use std::collections::BTreeMap;

    #[test]
    fn create_session_result_preserves_skill_diagnostics() {
        let run = meerkat_core::RunResult {
            text: "ok".to_string(),
            session_id: meerkat_core::SessionId::new(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
                source_health: meerkat_core::skills::SourceHealthSnapshot {
                    state: meerkat_core::skills::SourceHealthState::Degraded,
                    invalid_ratio: 0.2,
                    invalid_count: 1,
                    total_count: 5,
                    failure_streak: 3,
                    handshake_failed: false,
                },
                quarantined: vec![],
                collection_fault: None,
            }),
        };
        let wire: CreateSessionResult = run.into();
        assert!(wire.skill_diagnostics.is_some());
        assert_eq!(
            wire.skill_diagnostics.as_ref().unwrap().source_health.state,
            meerkat_core::skills::SourceHealthState::Degraded
        );
    }

    #[test]
    fn create_session_params_deferred_deserialization() {
        use super::{CreateSessionParams, InitialTurn};

        // With initial_turn: "deferred"
        let json = serde_json::json!({
            "prompt": "hello",
            "initial_turn": "deferred"
        });
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.initial_turn, Some(InitialTurn::Deferred));

        // With initial_turn: "run_immediately"
        let json = serde_json::json!({
            "prompt": "hello",
            "initial_turn": "run_immediately"
        });
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.initial_turn, Some(InitialTurn::RunImmediately));

        // Without initial_turn (default)
        let json = serde_json::json!({"prompt": "hello"});
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.initial_turn, None);
    }

    #[test]
    fn create_session_params_accept_injected_context() {
        use super::CreateSessionParams;
        use meerkat_core::ContentInput;

        // Optional injected_context parses as a list of content inputs; an
        // absent field stays None (pre-existing wire shape unchanged).
        let json = serde_json::json!({
            "prompt": "hello",
            "injected_context": [
                "ambient one",
                [{"type": "text", "text": "ambient two"}]
            ]
        });
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        let entries = params.injected_context.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], ContentInput::Text(text) if text == "ambient one"));
        assert!(matches!(&entries[1], ContentInput::Blocks(blocks) if blocks.len() == 1));

        let json = serde_json::json!({"prompt": "hello"});
        let params: CreateSessionParams = serde_json::from_value(json).unwrap();
        assert!(params.injected_context.is_none());
    }

    #[test]
    fn create_session_provider_param_fails_closed_for_unknown_provider() {
        let err = super::parse_provider_param("provider-shaped-cache-key").unwrap_err();

        assert!(err.contains("unknown provider"));
        assert!(err.contains("provider-shaped-cache-key"));
    }

    fn inherited_gemini_binding() -> (meerkat_core::Config, meerkat_core::AuthBindingRef) {
        let mut config = meerkat_core::Config::default();
        let mut global = meerkat_core::RealmConfigSection::default();
        global.backend.insert(
            "google".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "gemini".to_string(),
                backend_kind: "google_code_assist".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        global.auth.insert(
            "oauth".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "gemini".to_string(),
                auth_method: "google_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        global.binding.insert(
            "primary".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "google".to_string(),
                auth_profile: "oauth".to_string(),
                default_model: Some("gemini-3.1-flash-lite-preview".to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("global".to_string(), global);
        config.realm.insert(
            "dev".to_string(),
            meerkat_core::RealmConfigSection {
                parent: Some(meerkat_core::RealmId::global()),
                ..Default::default()
            },
        );
        let binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").expect("realm"),
            binding: meerkat_core::BindingId::parse("primary").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        (config, binding)
    }

    #[test]
    fn rpc_create_session_resolution_preserves_supported_config_pin() {
        let mut config = meerkat_core::Config::default();
        config.agent.model = "claude-opus-4-7".to_string();
        config.models.anthropic = "claude-sonnet-4-6".to_string();

        let resolved = resolve_rpc_create_session_model(&config, None, None, None)
            .expect("RPC lowers through shared resolver");
        assert_eq!(resolved.model, "claude-opus-4-7");
        assert_eq!(resolved.provider, meerkat_core::Provider::Anthropic);
    }

    #[test]
    fn rpc_create_session_resolution_preserves_inherited_binding_owner() {
        let (config, binding) = inherited_gemini_binding();
        let resolved = resolve_rpc_create_session_model(&config, None, None, Some(binding))
            .expect("RPC resolves inherited binding");

        assert_eq!(resolved.model, "gemini-3.1-flash-lite-preview");
        assert_eq!(resolved.provider, meerkat_core::Provider::Gemini);
        let owner = resolved.auth_binding.expect("owner-stamped binding");
        assert_eq!(owner.realm.as_str(), "global");
    }

    #[test]
    fn rpc_create_session_resolution_rejects_explicit_model_binding_mismatch() {
        let (config, binding) = inherited_gemini_binding();
        let err = resolve_rpc_create_session_model(
            &config,
            Some("gpt-5.5".to_string()),
            None,
            Some(binding),
        )
        .expect_err("known model owner must match binding provider");

        assert!(err.to_string().contains("registered for provider 'openai'"));
    }

    #[test]
    fn rpc_create_session_resolution_preserves_config_fault_ownership() {
        let config_error = meerkat::CreateSessionModelResolutionError::Config(
            meerkat_core::ConfigError::Validation("broken model registry".to_string()),
        );
        assert_eq!(
            create_session_model_resolution_error_code(&config_error),
            crate::error::INTERNAL_ERROR
        );
        let binding_error =
            meerkat::CreateSessionModelResolutionError::BindingMissingProviderDefault {
                realm: meerkat_core::RealmId::parse("dev").expect("realm"),
                binding: meerkat_core::BindingId::parse("primary").expect("binding"),
                provider: meerkat_core::Provider::SelfHosted,
            };
        assert_eq!(
            create_session_model_resolution_error_code(&binding_error),
            crate::error::INTERNAL_ERROR
        );

        assert_eq!(
            create_session_model_resolution_error_code(
                &meerkat::CreateSessionModelResolutionError::EmptyExplicitModel,
            ),
            crate::error::INVALID_PARAMS
        );
        assert_eq!(
            create_session_model_resolution_error_code(
                &meerkat::CreateSessionModelResolutionError::MissingProviderDefault {
                    provider: meerkat_core::Provider::SelfHosted,
                },
            ),
            crate::error::INVALID_PARAMS
        );
    }

    #[test]
    fn create_session_rejects_reserved_mob_peer_meta_labels() {
        let result = meerkat::surface::validate_public_peer_meta(Some(
            &meerkat_core::PeerMeta::default().with_label("mob_id", "team"),
        ));
        assert!(result.is_err(), "reserved mob peer labels must be rejected");
        let Err(err) = result else {
            unreachable!("asserted reserved mob peer labels are rejected above");
        };
        assert!(err.contains("mob_id"));
    }

    #[test]
    fn create_session_rejects_reserved_surface_metadata_keys() {
        let labels = BTreeMap::from([("meerkat.runtime_id".to_string(), "spoof".to_string())]);
        let result = meerkat::surface::validate_public_surface_metadata(Some(&labels), None);

        assert!(
            result.is_err(),
            "reserved surface metadata labels must be rejected"
        );
        assert!(result.unwrap_err().contains("meerkat.runtime_id"));
    }
}

/// Handle `session/archive`.
pub async fn handle_archive(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: ArchiveSessionParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    match runtime.archive_session(&session_id).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"archived": true})),
        Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
    }
}

/// Handle `session/inject_context`.
pub async fn handle_inject_context(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: InjectSystemContextParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    let req = meerkat_core::AppendSystemContextRequest {
        content: params.content,
        source: params.source,
        idempotency_key: params.idempotency_key,
        source_kind: meerkat_core::session::SystemContextSource::Normal,
        peer_response_terminal: None,
    };

    match runtime.append_system_context(&session_id, req).await {
        Ok(result) => RpcResponse::success(
            id,
            InjectSystemContextResult {
                status: result.status,
            },
        ),
        Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
    }
}
