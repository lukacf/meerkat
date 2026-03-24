//! `turn/*` method handlers.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat_contracts::SkillsParams;
use meerkat_core::ContentInput;
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
use meerkat::surface::{RequestContext, request_action};
use meerkat_runtime::SessionServiceRuntimeExt;

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

/// Parameters for `turn/start`.
#[derive(Debug, Deserialize)]
pub struct StartTurnParams {
    pub session_id: String,
    pub prompt: ContentInput,
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
    // -- Per-turn overrides ---------------------------------------------------
    /// Override model. On pending sessions, sets the model before materialization.
    /// On materialized sessions, hot-swaps the LLM client for the remainder of the session.
    #[serde(default)]
    pub model: Option<String>,
    /// Override provider. Typically inferred from model.
    #[serde(default)]
    pub provider: Option<String>,
    /// Override max_tokens. On pending sessions only.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Override system prompt. On pending sessions only.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Override output schema. On pending sessions only.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Override structured output retries. On pending sessions only.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Override provider-specific parameters. Applied alongside model/provider override.
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
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &meerkat_runtime::RuntimeSessionAdapter,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let params: StartTurnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, &runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    if let Some(context) = request_context.as_ref() {
        let runtime = Arc::clone(&runtime);
        let session_id = session_id.clone();
        context.replace_cancel_action(request_action(move || {
            let runtime = Arc::clone(&runtime);
            let session_id = session_id.clone();
            async move {
                let _ = runtime.interrupt(&session_id).await;
            }
        }));
        let _ = context.run_cancel_if_requested().await;
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

    let skill_refs = match canonical_skill_ids(&runtime, params.skill_refs, params.skill_references)
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

    // Lazy-register executor if not already registered.
    // This handles cases where session/create used deferred mode or
    // the session was created before the runtime adapter was active.
    if runtime_adapter.runtime_mode() == meerkat_runtime::RuntimeMode::V9Compliant
        && !runtime_adapter.contains_session(&session_id).await
    {
        let executor = Box::new(crate::session_executor::SessionRuntimeExecutor::new(
            runtime.clone(),
            session_id.clone(),
            notification_sink.clone(),
        ));
        runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
    }

    let result = match runtime
        .start_turn_via_runtime(
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
