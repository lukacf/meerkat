//! `meta/agent_memory/purge`: the call's params and its stdio handler.
//!
//! A purge blanks the content of a tombstoned record and of the earlier
//! versions it replaced, and keeps the rows. See
//! `memory/sqlite_store/purge.rs` for what it blanks and what it leaves.

use serde_json::Value;

use super::memory_methods::{
    MemoryParamsError, parse_agent_memory_identity, parse_agent_memory_realm,
};
use super::memory_scope_methods::{
    parse_scoped_memory_id, parse_shared_memory_scope, scoped_memory_invalid_params,
    scoped_memory_response,
};
use super::{JsonRpcError, JsonRpcResponse, agent_memory_rpc_error};
use crate::identity_first::AgentMemoryPurgeError;
use crate::memory::records::MemoryScope;

pub(crate) const PURGE_METHOD: &str = "meta/agent_memory/purge";

/// `meta/agent_memory/purge`: the record's scope and its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryPurgeRpcRequest {
    pub(crate) scope: MemoryScope,
    pub(crate) memory_id: String,
}

/// A request names an identity's record by `identity`, as
/// `meta/agent_memory/forget` does, or a shared record by `scope`: exactly
/// one of the two. With both, or neither, it is malformed.
pub(crate) fn parse_agent_memory_purge_params(
    params: &Value,
) -> Result<AgentMemoryPurgeRpcRequest, String> {
    let object = params
        .as_object()
        .ok_or_else(|| MemoryParamsError::ParamsMustBeObject.message())?;
    let realm = parse_agent_memory_realm(object).map_err(|err| err.message())?;
    let scope = match (
        object.contains_key("identity"),
        object.contains_key("scope"),
    ) {
        (true, false) => MemoryScope::Identity {
            identity: parse_agent_memory_identity(object)
                .map_err(|err| err.message())?
                .as_str()
                .to_string(),
            realm,
        },
        (false, true) => parse_shared_memory_scope(object, &realm)?,
        (true, true) => {
            return Err(
                "a purge names the record by `identity` or by `scope`, not by both".to_string(),
            );
        }
        (false, false) => {
            return Err(
                "a purge names the record by `identity` (an identity's own record) or by \
                 `scope` (a record of a shared scope)"
                    .to_string(),
            );
        }
    };
    let memory_id = parse_scoped_memory_id(object)?;
    Ok(AgentMemoryPurgeRpcRequest { scope, memory_id })
}

/// A purge's failure as both RPC surfaces answer it. A record that is not
/// tombstoned is refused with the invalid-params code and a typed `data`
/// (`kind`, the record's id and the kind of the status it has), so a caller
/// can forget the record first without reading the message; a malformed
/// request has the same code and no `data`.
pub(crate) fn purge_rpc_error(err: AgentMemoryPurgeError) -> JsonRpcError {
    match err {
        AgentMemoryPurgeError::NotTombstoned { memory_id, status } => {
            let data = serde_json::json!({
                "kind": AgentMemoryPurgeError::NOT_TOMBSTONED_KIND,
                "memory_id": memory_id,
                "status": status,
            });
            JsonRpcError {
                code: -32602,
                message: format!(
                    "agent memory purge failed: {}",
                    AgentMemoryPurgeError::NotTombstoned { memory_id, status }
                ),
                data: Some(data),
            }
        }
        AgentMemoryPurgeError::Memory(err) => agent_memory_rpc_error("purge", err),
    }
}

/// Where the record a call names by a member's name lies, when it lies
/// apart from the scope of the plain name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeptApart {
    /// In the scope kept under an owner and the member's name: the memory
    /// of a member private to that person.
    UnderAnOwner {
        scope: MemoryScope,
        owner: String,
        member: String,
    },
    /// In the scope the notes of the name were set aside under: no
    /// member's memory.
    SetAside { scope: MemoryScope },
}

impl KeptApart {
    /// The scope the record lies in.
    pub(crate) fn scope(&self) -> &MemoryScope {
        match self {
            Self::UnderAnOwner { scope, .. } | Self::SetAside { scope } => scope,
        }
    }
}

/// Where the record a call names by a member's name lies apart from the
/// plain name's scope, when `named` is an identity's scope and `kept`, the
/// scope the record lies in, is one of two: the scope kept under an owner
/// and a member's name ([`IdentityScopeKey::PrivateMember`]) whose member
/// name is the one `named` names, or the scope the notes of that name were
/// set aside under (the key the setting aside itself writes,
/// [`set_aside_scope_key`](crate::memory::sqlite_store::set_aside_scope_key)).
/// `None` for a shared scope, for a record under the plain name or under
/// any other key, and for an id no record holds. The purge and the forget
/// both ask here.
///
/// [`IdentityScopeKey::PrivateMember`]: crate::memory::records::IdentityScopeKey::PrivateMember
pub(crate) fn kept_apart(named: &MemoryScope, kept: Option<&MemoryScope>) -> Option<KeptApart> {
    use crate::memory::records::IdentityScopeKey;
    let MemoryScope::Identity {
        identity: named, ..
    } = named
    else {
        return None;
    };
    let kept = kept?;
    let MemoryScope::Identity { identity: key, .. } = kept else {
        return None;
    };
    match IdentityScopeKey::of(key) {
        IdentityScopeKey::PrivateMember { owner, member } if member == named.as_str() => {
            Some(KeptApart::UnderAnOwner {
                scope: kept.clone(),
                owner,
                member: member.to_string(),
            })
        }
        IdentityScopeKey::Reserved
            if *key == crate::memory::sqlite_store::set_aside_scope_key(named) =>
        {
            Some(KeptApart::SetAside {
                scope: kept.clone(),
            })
        }
        _ => None,
    }
}

/// The stdio handler: the purge is recorded as made by the application.
/// The host is the operator: a record of a member private to a person,
/// which lies under that person and the member's name, and a note that was
/// set aside from a name are purged under the name, with no caller rule.
pub(super) async fn handle_purge(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let request = match parse_agent_memory_purge_params(params) {
        Ok(request) => request,
        Err(message) => {
            return scoped_memory_response(response_id, Err(scoped_memory_invalid_params(message)));
        }
    };
    let kept = match identity_runtime
        .agent_memory_record_scope(request.scope.realm(), &request.memory_id)
        .await
    {
        Ok(kept) => kept,
        Err(err) => {
            return scoped_memory_response(response_id, Err(agent_memory_rpc_error("purge", err)));
        }
    };
    let scope = kept_apart(&request.scope, kept.as_ref())
        .map_or_else(|| request.scope.clone(), |apart| apart.scope().clone());
    let outcome = identity_runtime
        .purge_agent_memory(&scope, &request.memory_id)
        .await
        .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
        .map_err(purge_rpc_error);
    scoped_memory_response(response_id, outcome)
}
