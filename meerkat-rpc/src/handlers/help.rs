//! Handler for the dedicated `help/ask` JSON-RPC method.

use std::sync::Arc;

use serde_json::value::RawValue;

use super::session::CreateSessionParams;
use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;
use meerkat_core::ContentInput;

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

    // K17: construct the typed `session/create` params directly and delegate
    // to the shared typed entrypoint — no hand-shaped JSON round trip between
    // handlers.
    let create_params = CreateSessionParams {
        prompt: ContentInput::Text(prompt),
        initial_turn: None,
        model: request.model,
        provider: request.provider,
        max_tokens: request.max_tokens,
        system_prompt: Some(meerkat::help::help_system_prompt().to_string()),
        output_schema: None,
        structured_output_retries: None,
        hooks_override: None,
        enable_builtins: Some(false),
        enable_shell: Some(false),
        keep_alive: false,
        comms_name: None,
        peer_meta: None,
        enable_memory: Some(false),
        enable_schedule: None,
        enable_mob: Some(false),
        enable_web_search: None,
        tool_filter: None,
        enable_workgraph: None,
        budget_limits: None,
        provider_params: None,
        auth_binding: None,
        preload_skills: Some(meerkat::help::platform_preload_skills()),
        skill_refs: None,
        skill_references: None,
        labels: None,
        additional_instructions: None,
        app_context: None,
        shell_env: None,
        external_tools: None,
    };

    super::session::create_session_with_params(
        id,
        create_params,
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}
