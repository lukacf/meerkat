//! `turn/*` method handlers.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat_contracts::wire::runtime::WireRuntimeTurnMetadata;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::has_build_only_turn_overrides;
use meerkat_core::{ContentInput, OutputSchema, SurfaceSessionRecoveryOverrides};

use super::skills::reject_retired_skill_references;
use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::{InterruptNoopTarget, SessionRuntime};
use meerkat::surface::{RequestContext, request_action};
use meerkat_runtime::{RuntimeDriverError, RuntimeState, SessionServiceRuntimeExt};

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

/// Parameters for `turn/start`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartTurnParams {
    pub session_id: String,
    pub prompt: ContentInput,
    /// Override max_tokens for a deferred session's first materializing turn.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Override system prompt for a deferred session's first materializing turn.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Override structured output schema for a deferred session's first materializing turn.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Override structured output retries for a deferred session's first materializing turn.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Retired legacy string refs. Kept only to return a typed ingress error
    /// when old clients send the field.
    #[serde(default, deserialize_with = "reject_retired_skill_references")]
    pub skill_references: Option<Vec<String>>,
    /// Canonical per-turn runtime metadata carrier.
    #[serde(default)]
    pub turn_metadata: Option<WireRuntimeTurnMetadata>,
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
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
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

    let turn_metadata = params.turn_metadata.map(Into::into);
    let output_schema = match params.output_schema {
        Some(schema) => match OutputSchema::from_json_value(schema) {
            Ok(schema) => Some(schema),
            Err(err) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid output_schema: {err}"),
                );
            }
        },
        None => None,
    };
    let build_only_overrides = SurfaceSessionRecoveryOverrides {
        max_tokens: params.max_tokens,
        system_prompt: params.system_prompt,
        output_schema,
        structured_output_retries: params.structured_output_retries,
        ..Default::default()
    };
    let build_only_overrides =
        has_build_only_turn_overrides(&build_only_overrides).then_some(build_only_overrides);

    // Lazy-register executor if not already registered.
    // This handles cases where session/create used deferred mode or
    // the session was created before the runtime adapter was active.
    // Use session_has_executor (not contains_session) because sessions
    // registered via prepare_bindings() exist in the map but have no
    // RuntimeLoop — inputs would queue without being processed.
    if runtime_adapter.runtime_mode() == meerkat_runtime::RuntimeMode::V9Compliant
        && !runtime_adapter.session_has_executor(&session_id).await
    {
        #[cfg(feature = "mob")]
        {
            if let Err(err) = runtime
                .ensure_runtime_session_for_rotation(&session_id)
                .await
            {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("runtime executor registration failed: {err}"),
                );
            }
        }
        #[cfg(not(feature = "mob"))]
        {
            let executor = Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                runtime.clone(),
                session_id.clone(),
            ));
            runtime_adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
        }
    }

    let result = match runtime
        .start_turn_via_runtime(
            &session_id,
            params.prompt,
            mcp_event_tx,
            turn_metadata,
            build_only_overrides,
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

    match runtime
        .runtime_adapter()
        .hard_cancel_current_run(&session_id, "RPC turn interrupt")
        .await
    {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"interrupted": true})),
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Idle | RuntimeState::Attached,
        }) => RpcResponse::success(id, serde_json::json!({"interrupted": true})),
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed,
        })
        | Err(RuntimeDriverError::Destroyed) => {
            match runtime.interrupt_noop_target(&session_id).await {
                Ok(InterruptNoopTarget::Present) => {
                    RpcResponse::success(id, serde_json::json!({"interrupted": true}))
                }
                Ok(InterruptNoopTarget::Missing) => RpcResponse::error(
                    id,
                    error::SESSION_NOT_FOUND,
                    format!("Session not found: {session_id}"),
                ),
                Ok(InterruptNoopTarget::NotInterruptible(state)) => RpcResponse::error(
                    id,
                    error::INVALID_REQUEST,
                    format!("Session is not interruptible while runtime is {state}"),
                ),
                Err(err) => RpcResponse::error(id, err.code, err.message),
            }
        }
        Err(RuntimeDriverError::NotReady { state }) => RpcResponse::error(
            id,
            error::INVALID_REQUEST,
            format!("Session is not interruptible while runtime is {state}"),
        ),
        Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::StartTurnParams;

    #[test]
    fn start_turn_params_accept_build_only_deferred_overrides() {
        let params = serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "start",
            "system_prompt": "deferred system",
            "max_tokens": 256,
            "output_schema": { "type": "object" },
            "structured_output_retries": 2,
            "turn_metadata": {
                "model": "gpt-test"
            }
        });

        let params = serde_json::from_value::<StartTurnParams>(params)
            .expect("build-only deferred overrides must remain valid outside turn_metadata");
        assert_eq!(params.system_prompt.as_deref(), Some("deferred system"));
        assert_eq!(params.max_tokens, Some(256));
        assert!(params.output_schema.is_some());
        assert_eq!(params.structured_output_retries, Some(2));
        assert!(params.turn_metadata.is_some());
    }

    #[test]
    fn start_turn_params_reject_split_metadata_carriers() {
        for (field, value) in [
            ("keep_alive", serde_json::json!(true)),
            (
                "flow_tool_overlay",
                serde_json::json!({ "allowed_tools": ["read"] }),
            ),
            ("additional_instructions", serde_json::json!(["legacy"])),
            (
                "connection_ref",
                serde_json::json!({ "realm": "dev", "binding": "default" }),
            ),
        ] {
            let mut params = serde_json::json!({
                "session_id": "01234567-89ab-cdef-0123-456789abcdef",
                "prompt": "start"
            });
            params
                .as_object_mut()
                .unwrap()
                .insert(field.to_string(), value);
            let err = serde_json::from_value::<StartTurnParams>(params)
                .expect_err("split metadata field must be rejected");
            assert!(
                err.to_string().contains(field) || err.to_string().contains("unknown field"),
                "unexpected error for {field}: {err}"
            );
        }
    }

    #[test]
    fn start_turn_params_retired_skill_references_points_to_turn_metadata() {
        let params = serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "start",
            "skill_references": ["legacy/ref"]
        });
        let err = serde_json::from_value::<StartTurnParams>(params)
            .expect_err("retired skill references should be rejected");
        let message = err.to_string();
        assert!(
            message.contains("turn_metadata.skill_references"),
            "retired skill reference diagnostics must point to canonical metadata: {message}"
        );
        assert!(
            !message.contains("skill_refs"),
            "retired skill reference diagnostics must not advertise retired skill_refs: {message}"
        );
    }
}
