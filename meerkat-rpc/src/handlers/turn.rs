//! `turn/*` method handlers.

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat_contracts::SkillsParams;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::skills::{SkillKey, SkillRef};

use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
#[cfg(feature = "mob")]
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
    /// Structured refs for Skills V2.1.
    #[serde(default)]
    pub skill_refs: Option<Vec<SkillRef>>,
    /// Skill IDs to resolve and inject for this turn.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
    /// Optional per-turn tool visibility overlay.
    #[serde(default)]
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    /// Additional instruction sections prepended as system notices for this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Override host mode for this turn. Only applies to pending (deferred) sessions.
    #[serde(default)]
    pub host_mode: Option<bool>,
    // -- Resume-time overrides (pending/deferred sessions only) ---------------
    /// Override model for this session. Rejected on materialized sessions.
    #[serde(default)]
    pub model: Option<String>,
    /// Override provider for this session. Rejected on materialized sessions.
    #[serde(default)]
    pub provider: Option<String>,
    /// Override max_tokens for this session. Rejected on materialized sessions.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Override system prompt for this session. Rejected on materialized sessions.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Override output schema for this session. Rejected on materialized sessions.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Override structured output retries. Rejected on materialized sessions.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Override provider-specific parameters. Rejected on materialized sessions.
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
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

/// Collect per-turn override fields into a struct for `SessionRuntime::start_turn`.
#[derive(Debug, Default)]
pub struct TurnOverrides {
    pub host_mode: Option<bool>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub structured_output_retries: Option<u32>,
    pub provider_params: Option<serde_json::Value>,
}

impl TurnOverrides {
    fn is_empty(&self) -> bool {
        self.host_mode.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.max_tokens.is_none()
            && self.system_prompt.is_none()
            && self.output_schema.is_none()
            && self.structured_output_retries.is_none()
            && self.provider_params.is_none()
    }
}

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

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
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

    let skill_refs = match canonical_skill_ids(runtime, params.skill_refs, params.skill_references)
    {
        Ok(r) => r,
        Err(e) => {
            return RpcResponse::error(
                id,
                crate::error::INVALID_PARAMS,
                format!("Invalid skill_refs: {e}"),
            );
        }
    };

    let overrides = TurnOverrides {
        host_mode: params.host_mode,
        model: params.model,
        provider: params.provider,
        max_tokens: params.max_tokens,
        system_prompt: params.system_prompt,
        output_schema: params.output_schema,
        structured_output_retries: params.structured_output_retries,
        provider_params: params.provider_params,
    };

    let result = match runtime
        .start_turn(
            &session_id,
            params.prompt,
            event_tx,
            skill_refs,
            params.flow_tool_overlay,
            params.additional_instructions,
            if overrides.is_empty() {
                None
            } else {
                Some(overrides)
            },
        )
        .await
    {
        Ok(r) => r,
        Err(rpc_err) => {
            return RpcResponse::error(id, rpc_err.code, rpc_err.message);
        }
    };

    let mut response: TurnResult = result.into();
    response.session_ref = runtime
        .realm_id()
        .map(|realm| meerkat_contracts::format_session_ref(realm, &response.session_id));
    RpcResponse::success(id, response)
}

/// Handle `turn/interrupt`.
pub async fn handle_interrupt(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
    #[cfg(feature = "mob")] mob_state: &meerkat_mob_mcp::MobMcpState,
) -> RpcResponse {
    let params: InterruptParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    match runtime.interrupt(&session_id).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"interrupted": true})),
        #[cfg(feature = "mob")]
        Err(rpc_err) if rpc_err.code == error::SESSION_NOT_FOUND => {
            match mob_state.session_service().interrupt(&session_id).await {
                Ok(()) | Err(meerkat_core::service::SessionError::NotRunning { .. }) => {
                    RpcResponse::success(id, serde_json::json!({"interrupted": true}))
                }
                Err(err) => RpcResponse::error(id, error::SESSION_NOT_FOUND, err.to_string()),
            }
        }
        Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
    }
}
