//! `session/*` method handlers.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat::AgentBuildConfig;
use meerkat_core::Provider;
use meerkat_core::event::AgentEvent;

use super::{RpcResponseExt, UsageResult, parse_params};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

/// Parameters for `session/create`.
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

/// Result for `session/create`.
#[derive(Debug, Serialize)]
pub struct CreateSessionResult {
    pub session_id: String,
    pub text: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub usage: UsageResult,
}

/// Result for `session/list`.
#[derive(Debug, Serialize)]
pub struct ListSessionsResult {
    pub sessions: Vec<SessionInfoResult>,
}

/// Serializable session info.
#[derive(Debug, Serialize)]
pub struct SessionInfoResult {
    pub session_id: String,
    pub state: String,
}

/// Result for `session/read`.
#[derive(Debug, Serialize)]
pub struct ReadSessionResult {
    pub session_id: String,
    pub state: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `session/create`.
pub async fn handle_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
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

    let mut build_config = AgentBuildConfig::new(model_name);
    build_config.provider = provider;
    build_config.max_tokens = params.max_tokens;
    build_config.system_prompt = params.system_prompt;

    // Create the session
    let session_id = match runtime.create_session(build_config).await {
        Ok(sid) => sid,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

    // Set up event forwarding. The spawned task exits naturally when `event_tx`
    // is dropped at the end of the turn (the session task holds the only sender).
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(NOTIFICATION_CHANNEL_CAPACITY);
    let sink = notification_sink.clone();
    let sid_clone = session_id.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            sink.emit_event(&sid_clone, &event).await;
        }
    });

    // Start the initial turn
    let result = match runtime
        .start_turn(&session_id, params.prompt, event_tx)
        .await
    {
        Ok(r) => r,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

    let response = CreateSessionResult {
        session_id: session_id.to_string(),
        text: result.text,
        turns: result.turns,
        tool_calls: result.tool_calls,
        usage: UsageResult {
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
            cache_creation_tokens: result.usage.cache_creation_tokens,
            cache_read_tokens: result.usage.cache_read_tokens,
        },
    };

    RpcResponse::success(id, response)
}

/// Handle `session/list`.
pub async fn handle_list(id: Option<RpcId>, runtime: &SessionRuntime) -> RpcResponse {
    let sessions = runtime.list_sessions().await;
    let result = ListSessionsResult {
        sessions: sessions
            .into_iter()
            .map(|info| SessionInfoResult {
                session_id: info.session_id.to_string(),
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

    let session_id = match meerkat_core::types::SessionId::parse(&params.session_id) {
        Ok(sid) => sid,
        Err(_) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid session_id: {}", params.session_id),
            );
        }
    };

    match runtime.session_state(&session_id).await {
        Some(state) => {
            let result = ReadSessionResult {
                session_id: session_id.to_string(),
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

    let session_id = match meerkat_core::types::SessionId::parse(&params.session_id) {
        Ok(sid) => sid,
        Err(_) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid session_id: {}", params.session_id),
            );
        }
    };

    match runtime.archive_session(&session_id).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"archived": true})),
        Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
    }
}
