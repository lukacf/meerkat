//! `turn/*` method handlers.

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::{
    TurnMetadataOverride, legacy_override_from_split_fields, take_legacy_clear_bool,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat_contracts::SkillsParams;
use meerkat_core::ContentInput;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::skills::{SkillKey, SkillRef};

use super::skills::reject_retired_skill_references;
use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
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
///
/// `provider_params` and `auth_binding` carry the canonical Inherit/Set/Clear
/// tri-state via [`TurnMetadataOverride`]: `None` keeps the session's current
/// value, `Some(Set)` overrides it, `Some(Clear)` removes it. The legacy split
/// wire form (`provider_params` + `clear_provider_params: bool`) is still
/// accepted at the serde boundary and folded into the tri-state, rejecting a
/// `set + clear` payload there. This prevents cross-realm credential bleed in
/// multi-tenant setups (deferral §2 / Dogma §10).
#[derive(Debug)]
pub struct StartTurnParams {
    pub session_id: String,
    pub prompt: ContentInput,
    pub skill_refs: Option<Vec<SkillRef>>,
    pub skill_references: Option<Vec<String>>,
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    pub additional_instructions: Option<Vec<String>>,
    pub keep_alive: Option<bool>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub structured_output_retries: Option<u32>,
    pub provider_params: Option<TurnMetadataOverride<serde_json::Value>>,
    pub auth_binding: Option<TurnMetadataOverride<meerkat_core::AuthBindingRef>>,
}

#[derive(Debug, Deserialize)]
struct StartTurnParamsFields {
    session_id: String,
    prompt: ContentInput,
    #[serde(default)]
    skill_refs: Option<Vec<SkillRef>>,
    #[serde(default, deserialize_with = "reject_retired_skill_references")]
    skill_references: Option<Vec<String>>,
    #[serde(default)]
    flow_tool_overlay: Option<TurnToolOverlay>,
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
    #[serde(default)]
    keep_alive: Option<bool>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    output_schema: Option<serde_json::Value>,
    #[serde(default)]
    structured_output_retries: Option<u32>,
    #[serde(default)]
    provider_params: Option<TurnMetadataOverride<serde_json::Value>>,
    #[serde(default)]
    auth_binding: Option<TurnMetadataOverride<meerkat_core::AuthBindingRef>>,
}

impl<'de> Deserialize<'de> for StartTurnParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let mut raw = serde_json::Value::deserialize(deserializer)?;
        let (clear_provider_params, clear_auth_binding) = if let Some(object) = raw.as_object_mut()
        {
            (
                take_legacy_clear_bool(object, "clear_provider_params", &[])?,
                take_legacy_clear_bool(object, "clear_auth_binding", &[])?,
            )
        } else {
            (false, false)
        };
        let fields: StartTurnParamsFields =
            serde_json::from_value(raw).map_err(D::Error::custom)?;
        let provider_params = legacy_override_from_split_fields(
            fields.provider_params,
            clear_provider_params,
            "provider_params",
            "clear_provider_params",
        )?;
        let auth_binding = legacy_override_from_split_fields(
            fields.auth_binding,
            clear_auth_binding,
            "auth_binding",
            "clear_auth_binding",
        )?;
        Ok(Self {
            session_id: fields.session_id,
            prompt: fields.prompt,
            skill_refs: fields.skill_refs,
            skill_references: fields.skill_references,
            flow_tool_overlay: fields.flow_tool_overlay,
            additional_instructions: fields.additional_instructions,
            keep_alive: fields.keep_alive,
            model: fields.model,
            provider: fields.provider,
            max_tokens: fields.max_tokens,
            system_prompt: fields.system_prompt,
            output_schema: fields.output_schema,
            structured_output_retries: fields.structured_output_retries,
            provider_params,
            auth_binding,
        })
    }
}

/// Parameters for `turn/interrupt` — canonical wire type from contracts.
pub use meerkat_contracts::wire::InterruptParams;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result for `turn/start` — uses canonical wire type from contracts.
pub type TurnResult = meerkat_contracts::WireRunResult;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

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

/// Collect per-turn override fields into a struct for `SessionRuntime::start_turn`.
#[derive(Debug, Default)]
pub struct TurnOverrides {
    pub keep_alive: Option<bool>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub structured_output_retries: Option<u32>,
    pub provider_params: Option<TurnMetadataOverride<serde_json::Value>>,
    pub auth_binding: Option<TurnMetadataOverride<meerkat_core::AuthBindingRef>>,
}

impl TurnOverrides {
    pub(crate) fn is_empty(&self) -> bool {
        self.keep_alive.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.max_tokens.is_none()
            && self.system_prompt.is_none()
            && self.output_schema.is_none()
            && self.structured_output_retries.is_none()
            && self.provider_params.is_none()
            && self.auth_binding.is_none()
    }
}

/// Handle `turn/start`.
pub async fn handle_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let params: StartTurnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    start_turn_with_params(
        id,
        params,
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}

/// Typed `turn/start` entrypoint shared with identity-native callers
/// (`mob/turn_start`). Takes a fully-formed [`StartTurnParams`] so callers can
/// route typed params without re-serializing through a JSON `Map`.
pub async fn start_turn_with_params(
    id: Option<RpcId>,
    params: StartTurnParams,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, &runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    if let Some(context) = request_context.as_ref() {
        let runtime_adapter = Arc::clone(runtime_adapter);
        let session_id = session_id.clone();
        let install = context
            .install_cancel_action_or_cancelled(request_action(move || {
                let runtime_adapter = Arc::clone(&runtime_adapter);
                let session_id = session_id.clone();
                async move {
                    let _ = runtime_adapter
                        .hard_cancel_current_run(&session_id, "RPC request cancelled")
                        .await;
                }
            }))
            .await;
        if install == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            return RpcResponse::error(
                id,
                error::REQUEST_CANCELLED,
                "request cancelled before start",
            );
        }
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

    let skill_refs = match canonical_skill_ids(&runtime, params.skill_refs) {
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
        keep_alive: params.keep_alive,
        model: params.model,
        provider: params.provider,
        max_tokens: params.max_tokens,
        system_prompt: params.system_prompt,
        output_schema: params.output_schema,
        structured_output_retries: params.structured_output_retries,
        provider_params: params.provider_params,
        auth_binding: params.auth_binding,
    };

    let result = match runtime
        .start_turn_via_runtime(
            &session_id,
            params.prompt,
            mcp_event_tx,
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
        .map(|realm| meerkat_contracts::format_session_ref(&realm, &response.session_id));
    RpcResponse::success(id, response)
}

/// Handle `turn/interrupt`.
pub async fn handle_interrupt(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
    #[cfg(feature = "mob")] _mob_state: &meerkat_mob_mcp::MobMcpState,
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
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}
