//! `meta/agent_memory/forget` on the stdio surface: which scope a member's
//! name names for the host.
//!
//! A member that is private to a person keeps its agent memory in the
//! identity scope under that person and the member's name, and the notes
//! that were set aside from a plain name lie in that name's set-aside
//! scope. The host is the operator: it retires a record of either scope
//! under the name, whether or not an identity of that name is registered,
//! as it purges one ([`super::purge_methods`]). A record under a plain
//! name takes the registered identity's path, as before.

use super::memory_methods::AgentMemoryForgetRpcRequest;
use super::purge_methods::{KeptApart, kept_apart};
use crate::identity_first::agent_memory::{AgentMemoryError, AgentMemoryForgetResult};
use crate::memory::records::MemoryScope;

/// Where the record `memory_id` of `realm` lies when it lies apart from the
/// scope of the plain name `named` ([`kept_apart`]): under an owner and
/// that member name, or in the scope that name's notes were set aside
/// under. `None` for every other record, for an id no record holds, and
/// when the store cannot be asked: the caller then takes the path of the
/// plain name and answers as it did before.
pub(crate) async fn kept_apart_scope(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    realm: &str,
    named: &str,
    memory_id: &str,
) -> Option<KeptApart> {
    let kept = identity_runtime
        .agent_memory_record_scope(realm, memory_id)
        .await
        .ok()
        .flatten();
    let named = MemoryScope::Identity {
        realm: realm.to_string(),
        identity: named.to_string(),
    };
    kept_apart(&named, kept.as_ref())
}

/// Retire the record the request names, as the application. A record that
/// lies apart from the plain name's scope, under an owner and the member
/// name the request names or in that name's set-aside scope, is retired
/// there, with no caller rule and whether or not an identity of that name
/// is registered; every other request is the registered identity's forget.
pub(super) async fn forget_as_the_host(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    request: &AgentMemoryForgetRpcRequest,
) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
    match kept_apart_scope(
        identity_runtime,
        &request.realm,
        request.identity.as_str(),
        &request.memory_id,
    )
    .await
    {
        Some(apart) => {
            identity_runtime
                .forget_agent_memory_in_scope(apart.scope(), &request.memory_id)
                .await
        }
        None => {
            identity_runtime
                .forget_agent_memory(&request.realm, &request.identity, &request.memory_id)
                .await
        }
    }
}
