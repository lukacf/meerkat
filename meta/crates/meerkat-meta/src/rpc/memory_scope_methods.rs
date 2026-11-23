//! `meta/agent_memory/forget_in_scope`: the shared scope a memory call
//! names, the call's params and its stdio handler.
//!
//! `meta/agent_memory/forget` reaches an identity's own records only. This
//! call retires a record of a shared scope, which no call could before.

use serde_json::{Map, Value};

use super::memory_methods::{MemoryParamsError, parse_agent_memory_realm};
use super::{JSONRPC_VERSION, JsonRpcError, JsonRpcResponse, agent_memory_rpc_error};
use crate::memory::records::MemoryScope;

pub(crate) const FORGET_IN_SCOPE_METHOD: &str = "meta/agent_memory/forget_in_scope";

/// `meta/agent_memory/forget_in_scope`: the shared scope and the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMemoryForgetInScopeRpcRequest {
    pub(crate) scope: MemoryScope,
    pub(crate) memory_id: String,
}

pub(crate) fn parse_agent_memory_forget_in_scope_params(
    params: &Value,
) -> Result<AgentMemoryForgetInScopeRpcRequest, String> {
    let object = params
        .as_object()
        .ok_or_else(|| MemoryParamsError::ParamsMustBeObject.message())?;
    let realm = parse_agent_memory_realm(object).map_err(|err| err.message())?;
    let scope = parse_shared_memory_scope(object, &realm)?;
    let memory_id = parse_scoped_memory_id(object)?;
    Ok(AgentMemoryForgetInScopeRpcRequest { scope, memory_id })
}

/// The shared scope a request names in its `scope` param. An identity's own
/// scope is never named here (its calls take `identity`), and a person's
/// operator scope is reached by no host call.
pub(crate) fn parse_shared_memory_scope(
    object: &Map<String, Value>,
    realm: &str,
) -> Result<MemoryScope, String> {
    let scope = object
        .get("scope")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            "scope must be an object that names a shared memory scope, such as \
         {\"kind\": \"mob\", \"key\": \"<mob id>\"}"
                .to_string()
        })?;
    let kind = scope
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    match kind {
        "mob" => {
            let mob = scope
                .get("key")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| "scope.key must name the mob".to_string())?;
            Ok(MemoryScope::Mob {
                realm: realm.to_string(),
                mob: mob.to_string(),
            })
        }
        // The realm's own scope has no key.
        "realm" => Ok(MemoryScope::Realm {
            realm: realm.to_string(),
        }),
        other => Err(format!(
            "scope.kind '{other}' is not a shared memory scope this call reaches"
        )),
    }
}

pub(crate) fn parse_scoped_memory_id(object: &Map<String, Value>) -> Result<String, String> {
    object
        .get("memory_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| MemoryParamsError::MemoryIdRequired.message())
}

/// A scoped memory call's answer on the stdio surface: the result, or the
/// error in the form the memory calls already answer with.
pub(crate) fn scoped_memory_response(
    response_id: Value,
    outcome: Result<Value, JsonRpcError>,
) -> JsonRpcResponse {
    let (result, error) = match outcome {
        Ok(result) => (Some(result), None),
        Err(error) => (None, Some(error)),
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result,
        error,
    }
}

pub(crate) fn scoped_memory_invalid_params(message: String) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: format!("Invalid params: {message}"),
        data: None,
    }
}

/// The stdio handler: the removal is recorded as made by the application.
pub(super) async fn handle_forget_in_scope(
    identity_runtime: &crate::identity_first::IdentityRuntime,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let outcome = match parse_agent_memory_forget_in_scope_params(params) {
        Ok(request) => identity_runtime
            .forget_agent_memory_in_scope(&request.scope, &request.memory_id)
            .await
            .map(|result| serde_json::to_value(result).unwrap_or(Value::Null))
            .map_err(|err| agent_memory_rpc_error("forget", err)),
        Err(message) => Err(scoped_memory_invalid_params(message)),
    };
    scoped_memory_response(response_id, outcome)
}
