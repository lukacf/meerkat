//! `session/*` method handlers.

use meerkat::AgentBuildConfig;
use meerkat_contracts::SkillsParams;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::skills::{SkillKey, SkillRef};
use meerkat_core::{HookRunOverrides, OutputSchema, Provider};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::sync::Arc;
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

/// Parameters for `session/create`.
///
/// Mirrors the fields of [`AgentBuildConfig`] that are relevant for session
/// creation via the JSON-RPC surface. Optional fields default to `None`/`false`
/// so callers only need to provide `prompt`.
#[derive(Debug, Deserialize)]
pub struct CreateSessionParams {
    pub prompt: String,
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
    /// Enable built-in tools (task management, etc.)
    #[serde(default)]
    pub enable_builtins: bool,
    /// Enable shell tool (requires enable_builtins).
    #[serde(default)]
    pub enable_shell: bool,
    /// Run in host mode for inter-agent comms.
    #[serde(default)]
    pub host_mode: bool,
    /// Agent name for comms (required when host_mode is true).
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (description, labels).
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Enable sub-agent tools (fork, spawn).
    #[serde(default)]
    pub enable_subagents: bool,
    /// Enable semantic memory (memory_search tool + compaction indexing).
    #[serde(default)]
    pub enable_memory: bool,
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

/// Parameters for `session/read`.
#[derive(Debug, Deserialize)]
pub struct ReadSessionParams {
    pub session_id: String,
}

/// Parameters for `session/archive`.
#[derive(Debug, Deserialize)]
pub struct ArchiveSessionParams {
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result for `session/create` — uses canonical wire type from contracts.
pub type CreateSessionResult = meerkat_contracts::WireRunResult;

/// Result for `session/list`.
#[derive(Debug, Serialize)]
pub struct ListSessionsResult {
    pub sessions: Vec<SessionInfoResult>,
}

/// Serializable session info.
#[derive(Debug, Serialize)]
pub struct SessionInfoResult {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub state: String,
}

/// Result for `session/read`.
#[derive(Debug, Serialize)]
pub struct ReadSessionResult {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub state: String,
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
) -> RpcResponse {
    let params: CreateSessionParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let model_name = params
        .model
        .clone()
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string());
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
    build_config.override_builtins = Some(params.enable_builtins);
    build_config.override_shell = Some(params.enable_builtins && params.enable_shell);
    build_config.host_mode = params.host_mode;
    build_config.comms_name = params.comms_name;
    build_config.peer_meta = params.peer_meta;
    build_config.override_subagents = Some(params.enable_subagents);
    build_config.override_memory = Some(params.enable_memory);
    build_config.provider_params = params.provider_params;
    build_config.preload_skills = params
        .preload_skills
        .map(|ids| ids.into_iter().map(meerkat_core::skills::SkillId).collect());

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

    // Create the session
    let session_id = match runtime.create_session(build_config).await {
        Ok(sid) => sid,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

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

    // Start the initial turn
    let result = if params.host_mode {
        let runtime_for_turn = Arc::clone(&runtime);
        let sid_for_turn = session_id.clone();
        let event_tx_for_turn = event_tx.clone();
        let prompt_for_turn = params.prompt.clone();
        let skill_refs_for_turn = skill_refs.clone();
        tokio::spawn(async move {
            if let Err(rpc_err) = runtime_for_turn
                .start_turn(
                    &sid_for_turn,
                    prompt_for_turn,
                    event_tx_for_turn,
                    skill_refs_for_turn,
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
            .start_turn(&session_id, params.prompt, event_tx, skill_refs)
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
pub async fn handle_list(id: Option<RpcId>, runtime: &SessionRuntime) -> RpcResponse {
    let sessions = runtime.list_sessions().await;
    let result = ListSessionsResult {
        sessions: sessions
            .into_iter()
            .map(|info| SessionInfoResult {
                session_id: info.session_id.to_string(),
                session_ref: runtime
                    .realm_id()
                    .map(|realm| meerkat_contracts::format_session_ref(realm, &info.session_id)),
                state: info.state.as_str().to_string(),
            })
            .collect(),
    };
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

    match runtime.session_state(&session_id).await {
        Some(state) => {
            let result = ReadSessionResult {
                session_id: session_id.to_string(),
                session_ref: runtime
                    .realm_id()
                    .map(|realm| meerkat_contracts::format_session_ref(realm, &session_id)),
                state: state.as_str().to_string(),
            };
            RpcResponse::success(id, result)
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
