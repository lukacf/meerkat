//! `session/*` method handlers.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat::AgentBuildConfig;
use meerkat_contracts::SkillsParams;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::SessionQuery;
use meerkat_core::skills::{SkillKey, SkillRef};
use meerkat_core::{BudgetLimits, ContentInput, HookRunOverrides, OutputSchema, Provider};
use meerkat_runtime::SessionServiceRuntimeExt;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

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

/// Result for deferred `session/create` (no turn executed).
#[derive(Debug, Serialize)]
pub struct DeferredCreateResult {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
}

/// Parameters for `session/create`.
///
/// Mirrors the fields of [`AgentBuildConfig`] that are relevant for session
/// creation via the JSON-RPC surface. Optional fields default to `None`/`false`
/// so callers only need to provide `prompt`.
#[derive(Debug, Deserialize)]
pub struct CreateSessionParams {
    pub prompt: ContentInput,
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
    pub system_prompt: Option<String>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation (default: 2).
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    /// Run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable built-in tools. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Enable shell tool. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_shell: Option<bool>,
    /// Run in host mode for inter-agent comms.
    #[serde(default)]
    pub host_mode: bool,
    /// Agent name for comms (required when host_mode is true).
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (description, labels).
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Enable semantic memory. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable mob tools. Omit to use runtime defaults.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Explicit budget limits for this session.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimits>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
    /// Skill IDs to preload into the system prompt.
    #[serde(default)]
    pub preload_skills: Option<Vec<String>>,
    /// Skill IDs to resolve and inject for the first turn.
    #[serde(default)]
    pub skill_refs: Option<Vec<SkillRef>>,
    /// Legacy compatibility refs to resolve and inject for the first turn.
    #[serde(default)]
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

fn default_structured_output_retries() -> u32 {
    2
}

fn canonical_skill_ids(
    runtime: &SessionRuntime,
    skill_refs: Option<Vec<SkillRef>>,
    skill_references: Option<Vec<String>>,
) -> Result<Option<Vec<SkillKey>>, meerkat_core::skills::SkillError> {
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
        skill_references,
    };
    params.canonical_skill_keys_with_registry(&runtime.skill_identity_registry())
}

/// Parameters for `session/list` (all optional).
#[derive(Debug, Default, Deserialize)]
pub struct ListSessionsParams {
    /// Filter sessions by label key-value pairs (AND match).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Maximum number of sessions to return.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of sessions to skip.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Parameters for `session/read`.
#[derive(Debug, Deserialize)]
pub struct ReadSessionParams {
    pub session_id: String,
}

/// Parameters for `session/history`.
#[derive(Debug, Deserialize)]
pub struct ReadSessionHistoryParams {
    pub session_id: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Parameters for `session/archive`.
#[derive(Debug, Deserialize)]
pub struct ArchiveSessionParams {
    pub session_id: String,
}

/// Parameters for `session/inject_context`.
#[derive(Debug, Deserialize)]
pub struct InjectSystemContextParams {
    pub session_id: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result for `session/create` — uses canonical wire type from contracts.
pub type CreateSessionResult = meerkat_contracts::WireRunResult;

/// Result for `session/list` — uses canonical wire type from contracts.
#[derive(Debug, Serialize)]
pub struct ListSessionsResult {
    pub sessions: Vec<meerkat_contracts::WireSessionSummary>,
}

/// Result for `session/read` — uses canonical wire type from contracts.
pub type ReadSessionResult = meerkat_contracts::WireSessionInfo;

/// Result for `session/history` — uses canonical wire type from contracts.
pub type ReadSessionHistoryResult = meerkat_contracts::WireSessionHistory;

/// Result for `session/inject_context`.
#[derive(Debug, Serialize)]
pub struct InjectSystemContextResult {
    pub status: meerkat_core::AppendSystemContextStatus,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `session/create`.
pub async fn handle_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &meerkat_runtime::RuntimeSessionAdapter,
) -> RpcResponse {
    let params: CreateSessionParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    if let Err(err) = meerkat::surface::validate_public_peer_meta(params.peer_meta.as_ref()) {
        return RpcResponse::error(id, error::INVALID_PARAMS, err);
    }

    let runtime_default_model = if let Some(config_runtime) = runtime.config_runtime() {
        config_runtime
            .get()
            .await
            .ok()
            .map(|snapshot| snapshot.config.agent.model)
    } else {
        None
    };
    let model_name = params
        .model
        .clone()
        .or(runtime_default_model)
        .unwrap_or_else(|| {
            meerkat_models::default_model("anthropic")
                .unwrap_or("claude-sonnet-4-5")
                .to_string()
        });
    let provider = params.provider.as_deref().map(Provider::from_name);

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
    build_config.provider = provider;
    build_config.max_tokens = params.max_tokens;
    build_config.system_prompt = params.system_prompt;
    build_config.output_schema = output_schema;
    build_config.structured_output_retries = params.structured_output_retries;
    build_config.hooks_override = params.hooks_override.unwrap_or_default();
    build_config.override_builtins = params.enable_builtins;
    build_config.override_shell = params.enable_shell;
    build_config.host_mode = params.host_mode;
    build_config.comms_name = params.comms_name;
    build_config.peer_meta = params.peer_meta;
    build_config.override_memory = params.enable_memory;
    build_config.override_mob = params.enable_mob;
    build_config.budget_limits = params.budget_limits;
    build_config.provider_params = params.provider_params;
    build_config.additional_instructions = params.additional_instructions;
    build_config.app_context = params.app_context;
    build_config.shell_env = params.shell_env;
    build_config.preload_skills = params
        .preload_skills
        .map(|ids| ids.into_iter().map(meerkat_core::skills::SkillId).collect());

    // Wire callback tools if external_tools are provided or globally registered.
    // Inline (per-session) tools take precedence over globals with the same name.
    {
        let mut all_tools: Vec<meerkat_core::ToolDef> = params.external_tools.unwrap_or_default();
        let mut seen: std::collections::HashSet<String> =
            all_tools.iter().map(|t| t.name.clone()).collect();

        // Merge globally registered tools, skipping duplicates (inline wins).
        if let Ok(global) = runtime.registered_tools().read() {
            for tool in global.iter() {
                if seen.insert(tool.name.clone()) {
                    all_tools.push(tool.clone());
                }
            }
        }

        if !all_tools.is_empty()
            && let Some(tx) = runtime.callback_request_tx()
        {
            let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> =
                Arc::new(crate::callback_dispatcher::CallbackToolDispatcher::new(
                    all_tools,
                    tx,
                    runtime.callback_id_counter(),
                ));
            build_config.external_tools = Some(dispatcher);
        }
    }

    // Validate and canonicalize skill refs before creating a pending session.
    // This prevents invalid requests from consuming session slots.
    let skill_refs = match canonical_skill_ids(&runtime, params.skill_refs, params.skill_references)
    {
        Ok(r) => r,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid skill_refs: {e}"),
            );
        }
    };

    // Create the session. When deferred, stash the prompt for the first turn.
    let deferred_prompt = if params.initial_turn == Some(InitialTurn::Deferred) {
        Some(params.prompt.clone())
    } else {
        None
    };
    let session_id = match runtime
        .create_session(build_config, params.labels, deferred_prompt)
        .await
    {
        Ok(sid) => sid,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

    // Eagerly register executor BEFORE the first turn so the runtime loop
    // is available from the start. This replaces the post-response registration
    // that previously happened in the router.
    if runtime_adapter.runtime_mode() == meerkat_runtime::RuntimeMode::V9Compliant {
        let executor = Box::new(crate::session_executor::SessionRuntimeExecutor::new(
            runtime.clone(),
            session_id.clone(),
            notification_sink.clone(),
        ));
        runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
    }

    // Deferred mode: return session ID without running a turn.
    if params.initial_turn == Some(InitialTurn::Deferred) {
        let result = DeferredCreateResult {
            session_id: session_id.to_string(),
            session_ref: runtime
                .realm_id()
                .map(|realm| meerkat_contracts::format_session_ref(realm, &session_id)),
        };
        return RpcResponse::success(id, result);
    }

    // Set up event forwarding. The spawned task exits naturally when `event_tx`
    // is dropped at the end of the turn (the session task holds the only sender).
    let (event_tx, mut event_rx) =
        mpsc::channel::<EventEnvelope<AgentEvent>>(NOTIFICATION_CHANNEL_CAPACITY);
    let sink = notification_sink.clone();
    let sid_clone = session_id.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            sink.emit_event(&sid_clone, &event).await;
        }
    });

    // Start the initial turn — route through runtime for V9 consistency
    let result = if params.host_mode {
        let runtime_for_turn = Arc::clone(&runtime);
        let sid_for_turn = session_id.clone();
        let event_tx_for_turn = event_tx.clone();
        let prompt_for_turn = params.prompt.clone();
        let skill_refs_for_turn = skill_refs.clone();
        tokio::spawn(async move {
            if let Err(rpc_err) = runtime_for_turn
                .start_turn_via_runtime(
                    &sid_for_turn,
                    prompt_for_turn,
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
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: None,
        }
    } else {
        match runtime
            .start_turn_via_runtime(
                &session_id,
                params.prompt,
                event_tx,
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
        .map(|realm| meerkat_contracts::format_session_ref(realm, &response.session_id));
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

    let query = SessionQuery {
        labels: list_params.labels,
        ..Default::default()
    };

    let mut sessions: Vec<meerkat_contracts::WireSessionSummary> = runtime
        .list_sessions_rich(query)
        .await
        .into_iter()
        .map(|mut ws| {
            ws.session_ref = runtime
                .realm_id()
                .map(|realm| meerkat_contracts::format_session_ref(realm, &ws.session_id));
            ws
        })
        .collect();

    // Apply pagination.
    let offset = list_params.offset.unwrap_or(0);
    if offset > 0 {
        sessions = sessions.into_iter().skip(offset).collect();
    }
    if let Some(limit) = list_params.limit {
        sessions.truncate(limit);
    }

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
        Some(mut info) => {
            info.session_ref = runtime
                .realm_id()
                .map(|realm| meerkat_contracts::format_session_ref(realm, &info.session_id));
            RpcResponse::success(id, info)
        }
        None => RpcResponse::error(
            id,
            error::SESSION_NOT_FOUND,
            format!("Session not found: {session_id}"),
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::items_after_test_module)]
mod tests {
    use super::CreateSessionResult;

    #[test]
    fn create_session_result_preserves_skill_diagnostics() {
        let run = meerkat_core::RunResult {
            text: "ok".to_string(),
            session_id: meerkat_core::SessionId::new(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
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
        text: params.text,
        source: params.source,
        idempotency_key: params.idempotency_key,
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
