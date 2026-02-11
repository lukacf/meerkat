//! `turn/*` method handlers.

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat_core::event::AgentEvent;

use super::{RpcResponseExt, parse_params};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

/// Parameters for `turn/start`.
#[derive(Debug, Deserialize)]
pub struct StartTurnParams {
    pub session_id: String,
    pub prompt: String,
    /// Skill IDs to resolve and inject for this turn.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
}

/// Parameters for `turn/interrupt`.
#[derive(Debug, Deserialize)]
pub struct InterruptParams {
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result for `turn/start` — uses canonical wire type from contracts.
pub type TurnResult = meerkat_contracts::WireRunResult;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `turn/start`.
pub async fn handle_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
    notification_sink: &NotificationSink,
) -> RpcResponse {
    let params: StartTurnParams = match parse_params(params) {
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

    let skill_refs = params.skill_references.map(|ids| {
        ids.into_iter().map(meerkat_core::skills::SkillId).collect()
    });
    let result = match runtime
        .start_turn(&session_id, params.prompt, event_tx, skill_refs)
        .await
    {
        Ok(r) => r,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

    let response: TurnResult = result.into();
    RpcResponse::success(id, response)
}

/// Handle `turn/interrupt`.
pub async fn handle_interrupt(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: InterruptParams = match parse_params(params) {
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

    match runtime.interrupt(&session_id).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"interrupted": true})),
        Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
    }
}
