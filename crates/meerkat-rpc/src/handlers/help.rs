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

    // Like every help entry point, help/ask builds its session from the one
    // shared help session: the embedded skills whole in the system prompt and
    // the read-only reference tools as the session's only tools.
    let help = meerkat::help::help_session();
    let create_params = help_create_session_params(request, prompt, help.system_prompt);

    // K17: delegate the typed `session/create` params to the shared typed
    // entrypoint, with no callback tools and no configured MCP servers. The
    // help tools come with the launch policy that admits only them.
    super::session::create_session_with_params_on_route_and_configured_mcp(
        id,
        create_params,
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
        None,
        Vec::new(),
        Some((help.tools, help.tool_access_policy)),
    )
    .await
}

/// The typed `session/create` params of a help session: the help system
/// prompt, no preloaded skills, and every tool category off.
fn help_create_session_params(
    request: meerkat_contracts::HelpRequest,
    prompt: String,
    system_prompt: String,
) -> CreateSessionParams {
    CreateSessionParams {
        prompt: ContentInput::Text(prompt),
        injected_context: None,
        transient_turn_context: None,
        // Today's behaviour: the help turn's events are written as
        // session/event.
        session_events: None,
        initial_turn: None,
        model: request.model,
        provider: request.provider,
        max_tokens: request.max_tokens,
        system_prompt: meerkat::SystemPromptOverride::Set(system_prompt),
        output_schema: None,
        structured_output_retries: None,
        hooks_override: None,
        enable_builtins: Some(false),
        enable_shell: Some(false),
        keep_alive: false,
        comms_name: None,
        peer_meta: None,
        enable_memory: Some(false),
        enable_schedule: Some(false),
        enable_mob: Some(false),
        enable_web_search: Some(false),
        tool_filter: None,
        enable_workgraph: Some(false),
        budget_limits: None,
        provider_params: None,
        auth_binding: None,
        preload_skills: None,
        skill_refs: None,
        skill_references: None,
        labels: None,
        additional_instructions: None,
        app_context: None,
        shell_env: None,
        external_tools: None,
    }
}
