//! The Console's door to `meta/agent_memory/forget`: which scope a member's
//! name names for the caller, and the handler.

use super::*;
use crate::rpc::purge_methods::KeptApart;

/// The scope apart from the plain name's in which the Console retires the
/// record the request names, or `None` for the path of the plain name.
///
/// A member that is private to a person keeps its agent memory under that
/// person and the member's name. When the record the request names lies
/// there
/// ([`kept_apart_scope`](crate::rpc::forget_methods::kept_apart_scope)), it
/// is retired there for an access administrator, and for that owner while
/// the Console knows the member, where the access rules allow them
/// `agent.memory.delete` on that member
/// ([`memory_panel_owned_member_allowed`]). That is not the owner's answer
/// once the member has ended and the Console holds nothing about it
/// ([`AccessView::withholds_unknown_member`]): a member the Console does
/// not know is answered to her as to everybody who is no administrator.
///
/// A record in the scope the name's notes were set aside under is no
/// member's memory: it is retired there for an access administrator alone,
/// as the memory panel shows such a scope to administrators alone.
///
/// Every other caller takes the path of the plain name, as before: the
/// answer is the one for a member that holds no such record, and the
/// record stays. A caller of a Console with no access view is kept apart
/// from nobody.
async fn kept_scope_for(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    caller: MemoryPanelCaller<'_>,
    request: &crate::rpc::memory_methods::AgentMemoryForgetRpcRequest,
) -> Option<crate::memory::records::MemoryScope> {
    let apart = crate::rpc::forget_methods::kept_apart_scope(
        identity_runtime,
        &request.realm,
        request.identity.as_str(),
        &request.memory_id,
    )
    .await?;
    let retires = match &apart {
        KeptApart::UnderAnOwner { owner, member, .. } => caller.view.is_none_or(|view| {
            let known_owner = view.private_owner_key() == Some(owner.as_str())
                && !view.withholds_unknown_member(&namespace_console_identity(
                    member,
                    caller.namespace,
                ));
            (view.is_admin() || known_owner)
                && memory_panel_owned_member_allowed(
                    caller,
                    view,
                    ACTION_AGENT_MEMORY_DELETE,
                    owner,
                    member,
                )
        }),
        KeptApart::SetAside { .. } => caller.view.is_none_or(AccessView::is_admin),
    };
    retires.then(|| apart.scope().clone())
}

/// Retire a record as the Console's caller: the author is the operator,
/// named by the authenticated principal only. A record that lies apart
/// from the plain name's scope is retired in the scope [`kept_scope_for`]
/// names for the caller; every other request is the registered identity's
/// forget.
pub(super) async fn handle_console_forget(
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    caller: MemoryPanelCaller<'_>,
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
    let request = match parse_agent_memory_forget_params(params) {
        Ok(request) => request,
        Err(err) => {
            return invalid_params(response_id, format!("Invalid params: {}", err.message()));
        }
    };
    let author = console_memory_author(authenticated_principal);
    let outcome = match kept_scope_for(identity_runtime, caller, &request).await {
        Some(scope) => {
            identity_runtime
                .forget_agent_memory_in_scope_by(&scope, &request.memory_id, author)
                .await
        }
        None => {
            identity_runtime
                .forget_agent_memory_by(
                    &request.realm,
                    &request.identity,
                    &request.memory_id,
                    author,
                )
                .await
        }
    };
    match outcome {
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
    }
}
