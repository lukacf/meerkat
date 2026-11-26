//! The Console's door to `meta/agent_memory/purge`: its grant and its
//! handler.

use super::*;

/// The grant a purge needs: `agent.memory.admin`, on the identity the
/// request names for an identity's record, and unscoped (no resource) for a
/// record of a shared scope. A request that names neither is held to the
/// unscoped grant, so a caller without it learns nothing from the parse.
pub(super) fn purge_requirements(
    identity: Option<String>,
    params: &Value,
) -> Vec<(&'static str, Option<String>)> {
    let target = if params.get("identity").is_some() {
        identity
    } else {
        None
    };
    vec![(crate::access::ACTION_AGENT_MEMORY_ADMIN, target)]
}

/// Whether `method` is the purge, which names the identity whose memory
/// scope holds the record. The member resolution then treats a member the
/// Console does not know as the memory reads do
/// ([`AccessView::withholds_unknown_member`]): for a caller the Console
/// keeps apart from other people it is answered as a member that does not
/// exist, so the purge blanks nothing there and its answer says nothing of
/// what that scope holds. A member that ended before this process read who
/// it was private to may have been private; an administrator, and the one
/// operator of a Console that identifies nobody, are withheld nothing. The
/// host call has no such rule: it is the operator's door to the memory of
/// an agent that was removed.
pub(super) fn names_an_identity_scope(method: &str) -> bool {
    method == crate::rpc::purge_methods::PURGE_METHOD
}

/// Whether the Console offers the purge to the caller of `view`: only to
/// one who holds its grant on some agent or with no agent named. The
/// requirement names an agent for an identity's record, so the grant
/// filter of the advertised methods, which judges only requirements that
/// name none, would offer it to everybody.
pub(super) fn offered_to(view: &AccessView) -> bool {
    view.may_perform_anywhere(crate::access::ACTION_AGENT_MEMORY_ADMIN)
}

/// The scope a Console purge is made in: the scope the call names, or a
/// scope apart from it that the record lies in
/// ([`kept_apart`](crate::rpc::purge_methods::kept_apart)). The scope kept
/// under an owner and the member name the call names is taken for a caller
/// who is that owner or an access administrator and whom the rules allow
/// `agent.memory.admin` on that member
/// ([`memory_panel_owned_member_allowed`]). The scope the name's notes
/// were set aside under is no member's memory: it is taken for an access
/// administrator alone, as the memory panel shows such a scope to
/// administrators alone. Every other caller purges in the named scope,
/// where the record is not: the store answers as for a record that does
/// not exist and blanks nothing, and so never tells such a caller that the
/// record exists. A caller of a Console with no access view is kept apart
/// from nobody.
async fn purge_scope(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    caller: MemoryPanelCaller<'_>,
    request: &crate::rpc::purge_methods::AgentMemoryPurgeRpcRequest,
) -> Result<
    crate::memory::records::MemoryScope,
    crate::identity_first::agent_memory::AgentMemoryError,
> {
    let kept = identity_runtime
        .agent_memory_record_scope(request.scope.realm(), &request.memory_id)
        .await?;
    use crate::rpc::purge_methods::KeptApart;
    let Some(apart) = crate::rpc::purge_methods::kept_apart(&request.scope, kept.as_ref()) else {
        return Ok(request.scope.clone());
    };
    let purges = match &apart {
        KeptApart::UnderAnOwner { owner, member, .. } => caller.view.is_none_or(|view| {
            (view.is_admin() || view.private_owner_key() == Some(owner.as_str()))
                && memory_panel_owned_member_allowed(
                    caller,
                    view,
                    crate::access::ACTION_AGENT_MEMORY_ADMIN,
                    owner,
                    member,
                )
        }),
        KeptApart::SetAside { .. } => caller.view.is_none_or(AccessView::is_admin),
    };
    Ok(if purges {
        apart.scope().clone()
    } else {
        request.scope.clone()
    })
}

/// Purge a retired record's content as the Console's caller: the author is
/// the operator, named by the authenticated principal only. The record is
/// purged in the scope [`purge_scope`] names for the caller.
pub(super) async fn handle_console_purge(
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
    let request = match crate::rpc::purge_methods::parse_agent_memory_purge_params(params) {
        Ok(request) => request,
        Err(message) => {
            return invalid_params(response_id, format!("Invalid params: {message}"));
        }
    };
    let scope = match purge_scope(identity_runtime, caller, &request).await {
        Ok(scope) => scope,
        Err(err) => {
            return response_value(
                response_id,
                None,
                Some(crate::rpc::agent_memory_rpc_error("purge", err)),
            );
        }
    };
    match identity_runtime
        .purge_agent_memory_by(
            &scope,
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
            Some(crate::rpc::purge_methods::purge_rpc_error(err)),
        ),
    }
}
