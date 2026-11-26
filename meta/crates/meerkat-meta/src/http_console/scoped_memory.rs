//! The Console's door to `meta/agent_memory/forget_in_scope`: its grant and
//! its handler.

use super::*;

/// The grant a removal from a shared scope needs. The mob's scope has an
/// action of its own, `mob.memory.delete`, which names no agent. The
/// realm's scope rides the unscoped `agent.memory.delete`, as its reads
/// ride the unscoped `agent.memory.read`. A request whose scope does not
/// parse is held to the mob action, so a caller without it learns nothing
/// from the parse.
pub(super) fn forget_in_scope_requirements(params: &Value) -> Vec<(&'static str, Option<String>)> {
    let kind = params
        .get("scope")
        .and_then(|scope| scope.get("kind"))
        .and_then(Value::as_str)
        .map(str::trim);
    match kind {
        Some("realm") => vec![(crate::access::ACTION_AGENT_MEMORY_DELETE, None)],
        _ => vec![(crate::access::ACTION_MOB_MEMORY_DELETE, None)],
    }
}

/// Retire a record of a shared scope as the Console's caller: the author
/// is the operator, named by the authenticated principal only.
pub(super) async fn handle_console_forget_in_scope(
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    authenticated_principal: Option<&str>,
    params: &Value,
    response_id: Value,
) -> Value {
    let Some(identity_runtime) = identity_runtime else {
        return response_value(
            response_id,
            None,
            Some(JsonRpcError {
                code: -32601,
                message: "agent memory is not configured".to_string(),
                data: None,
            }),
        );
    };
    match crate::rpc::memory_scope_methods::parse_agent_memory_forget_in_scope_params(params) {
        Ok(request) => match identity_runtime
            .forget_agent_memory_in_scope_by(
                &request.scope,
                &request.memory_id,
                console_memory_author(authenticated_principal),
            )
            .await
        {
            Ok(result) => response_value(
                response_id,
                Some(serde_json::to_value(result).unwrap_or(Value::Null)),
                None,
            ),
            Err(err) => response_value(
                response_id,
                None,
                Some(crate::rpc::agent_memory_rpc_error("forget", err)),
            ),
        },
        Err(message) => invalid_params(response_id, format!("Invalid params: {message}")),
    }
}
