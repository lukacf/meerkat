//! `turn/*` method handlers.

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
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
/// value, `Some(Set)` overrides it, `Some(Clear)` removes it. Unknown fields
/// (including the retired `clear_*` split wire form) fail closed at the serde
/// boundary. This prevents cross-realm credential bleed in multi-tenant
/// setups (deferral §2 / Dogma §10).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartTurnParams {
    pub session_id: String,
    pub prompt: ContentInput,
    #[serde(default)]
    pub skill_refs: Option<Vec<SkillRef>>,
    #[serde(default, deserialize_with = "reject_retired_skill_references")]
    pub skill_references: Option<Vec<String>>,
    #[serde(default)]
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default)]
    pub keep_alive: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    #[serde(default)]
    pub provider_params: Option<
        TurnMetadataOverride<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    >,
    #[serde(default)]
    pub auth_binding: Option<TurnMetadataOverride<meerkat_core::AuthBindingRef>>,
}

/// Parameters for `turn/interrupt` — canonical wire type from contracts.
pub use meerkat_contracts::wire::InterruptParams;

/// Result for `turn/interrupt` — canonical wire type from contracts.
///
/// Carries the generated interrupt public-result tri-state: a live run was
/// cancelled (`interrupted`), or a staged (not-yet-promoted) session interrupt
/// was a typed no-op terminal (`staged_noop`). NotFound/SessionBusy/Conflict
/// remain typed JSON-RPC errors. This mirrors the REST surface exactly — the
/// previous hand-shaped `{"interrupted": true}` claimed a live cancellation
/// for every non-error outcome.
pub use meerkat_contracts::wire::{InterruptResult, WireInterruptOutcome};

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
    pub provider_params: Option<
        TurnMetadataOverride<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    >,
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

    // `SessionRuntime::interrupt` surfaces the generated interrupt
    // public-result tri-state (Interrupted vs StagedNoop) for non-error
    // outcomes; NotFound/SessionBusy/Conflict arrive as typed RpcErrors.
    // The handler serializes the contracts `InterruptResult` wire type —
    // no hand-shaped payloads — so RPC reports a staged-session no-op
    // honestly instead of fabricating a live cancellation.
    match runtime.interrupt(&session_id).await {
        Ok(outcome) => RpcResponse::success(
            id,
            InterruptResult::from_outcome(session_id.to_string(), outcome),
        ),
        Err(err) => RpcResponse::error(id, err.code, err.message),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod interrupt_wire_tests {
    use super::{InterruptResult, WireInterruptOutcome};

    /// K17 regression: a staged-session interrupt must serialize as the
    /// honest tri-state (`interrupted: false`, `result: staged_noop`) — RPC
    /// and REST share the same generated wire contract, so the previous RPC
    /// divergence (`{"interrupted": true}` for every Ok) cannot reappear.
    #[test]
    fn staged_noop_interrupt_serializes_honest_tri_state() {
        let staged = InterruptResult::from_outcome(
            "session-1".to_string(),
            WireInterruptOutcome::StagedNoop,
        );
        let value = serde_json::to_value(&staged).unwrap();
        assert_eq!(value["session_id"], "session-1");
        assert_eq!(value["interrupted"], false);
        assert_eq!(value["result"], "staged_noop");

        let interrupted = InterruptResult::from_outcome(
            "session-1".to_string(),
            WireInterruptOutcome::Interrupted,
        );
        let value = serde_json::to_value(&interrupted).unwrap();
        assert_eq!(value["interrupted"], true);
        assert_eq!(value["result"], "interrupted");
    }
}
