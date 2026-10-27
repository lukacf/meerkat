//! RPC capability boundary probing for module discovery.

use super::*;

pub fn run_rpc_capabilities_boundary_once(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<RpcCapabilities, RpcRuntimeError> {
    let line =
        run_process_json_line(command, args, env, timeout).map_err(RpcRuntimeError::Process)?;
    parse_rpc_capabilities(&line).map_err(RpcRuntimeError::Capabilities)
}

pub fn route_module_call_rpc_json(
    runtime: &MetaRuntimeHandle,
    request_json: &str,
    timeout: Duration,
) -> Result<String, RpcRouteError> {
    let request: ModuleRouteRequest =
        serde_json::from_str(request_json).map_err(|_| RpcRouteError::InvalidRequest)?;
    let response = route_module_call(runtime, &request, timeout).map_err(RpcRouteError::Route)?;
    serde_json::to_string(&response).map_err(|_| RpcRouteError::InvalidResponse)
}

pub fn route_module_call_rpc_subprocess(
    runtime: &MetaRuntimeHandle,
    command: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, RpcRouteError> {
    let request_json = run_process_json_line(command, args, env, timeout)
        .map_err(RpcRouteError::BoundaryProcess)?;
    route_module_call_rpc_json(runtime, &request_json, timeout)
}
