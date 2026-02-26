//! `mcp/*` live operation handlers.

use serde_json::value::RawValue;

#[cfg(feature = "mcp")]
use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// Handle `mcp/add` with contract-typed params and placeholder response.
pub async fn handle_add(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    #[cfg(not(feature = "mcp"))]
    {
        let _ = (params, runtime);
        RpcResponse::error(
            id,
            error::METHOD_NOT_FOUND,
            "mcp/add requires the mcp feature",
        )
    }

    #[cfg(feature = "mcp")]
    {
        let params: meerkat_contracts::McpAddParams = match parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp.with_id(id),
        };

        let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime)
        {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        if params.server_name.trim().is_empty() {
            return RpcResponse::error(id, error::INVALID_PARAMS, "server_name cannot be empty");
        }

        if let Err(err) = runtime
            .mcp_stage_add(
                &session_id,
                params.server_name.clone(),
                params.server_config.clone(),
            )
            .await
        {
            return RpcResponse::error(id, err.code, err.message);
        }

        let response = meerkat_contracts::McpLiveOpResponse {
            session_id: params.session_id,
            operation: meerkat_contracts::McpLiveOperation::Add,
            server_name: Some(params.server_name),
            status: meerkat_contracts::McpLiveOpStatus::Staged,
            persisted: params.persisted,
            applied_at_turn: None,
        };
        RpcResponse::success(id, response)
    }
}

/// Handle `mcp/remove` with contract-typed params and placeholder response.
pub async fn handle_remove(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    #[cfg(not(feature = "mcp"))]
    {
        let _ = (params, runtime);
        RpcResponse::error(
            id,
            error::METHOD_NOT_FOUND,
            "mcp/remove requires the mcp feature",
        )
    }

    #[cfg(feature = "mcp")]
    {
        let params: meerkat_contracts::McpRemoveParams = match parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp.with_id(id),
        };

        let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime)
        {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        if params.server_name.trim().is_empty() {
            return RpcResponse::error(id, error::INVALID_PARAMS, "server_name cannot be empty");
        }

        if let Err(err) = runtime
            .mcp_stage_remove(&session_id, params.server_name.clone())
            .await
        {
            return RpcResponse::error(id, err.code, err.message);
        }

        let response = meerkat_contracts::McpLiveOpResponse {
            session_id: params.session_id,
            operation: meerkat_contracts::McpLiveOperation::Remove,
            server_name: Some(params.server_name),
            status: meerkat_contracts::McpLiveOpStatus::Staged,
            persisted: params.persisted,
            applied_at_turn: None,
        };
        RpcResponse::success(id, response)
    }
}

/// Handle `mcp/reload` with contract-typed params and placeholder response.
pub async fn handle_reload(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    #[cfg(not(feature = "mcp"))]
    {
        let _ = (params, runtime);
        RpcResponse::error(
            id,
            error::METHOD_NOT_FOUND,
            "mcp/reload requires the mcp feature",
        )
    }

    #[cfg(feature = "mcp")]
    {
        let params: meerkat_contracts::McpReloadParams = match parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp.with_id(id),
        };

        let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime)
        {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        if let Some(server_name) = params.server_name.as_ref()
            && server_name.trim().is_empty()
        {
            return RpcResponse::error(id, error::INVALID_PARAMS, "server_name cannot be empty");
        }

        if let Err(err) = runtime
            .mcp_stage_reload(&session_id, params.server_name.clone())
            .await
        {
            return RpcResponse::error(id, err.code, err.message);
        }

        let response = meerkat_contracts::McpLiveOpResponse {
            session_id: params.session_id,
            operation: meerkat_contracts::McpLiveOperation::Reload,
            server_name: params.server_name,
            status: meerkat_contracts::McpLiveOpStatus::Staged,
            persisted: params.persisted,
            applied_at_turn: None,
        };
        RpcResponse::success(id, response)
    }
}
