//! Handler for the dedicated `help/ask` JSON-RPC method.

use std::sync::Arc;

use serde_json::json;
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;

pub async fn handle_ask(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<meerkat::surface::RequestContext>,
) -> RpcResponse {
    let request: meerkat_contracts::HelpRequest = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let prompt = match meerkat::help::render_help_prompt(&request) {
        Ok(prompt) => prompt,
        Err(err) => return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    };

    let create_params = json!({
        "prompt": prompt,
        "system_prompt": meerkat::help::help_system_prompt(),
        "model": request.model,
        "provider": request.provider,
        "max_tokens": request.max_tokens,
        "preload_skills": meerkat::help::platform_preload_skills(),
        "enable_builtins": false,
        "enable_shell": false,
        "enable_memory": false,
        "enable_mob": false,
    });
    let raw = match serde_json::value::to_raw_value(&create_params) {
        Ok(raw) => raw,
        Err(err) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("failed to encode help session params: {err}"),
            );
        }
    };

    super::session::handle_create(
        id,
        Some(raw.as_ref()),
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}
