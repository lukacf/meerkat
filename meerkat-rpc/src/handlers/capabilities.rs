//! Handler for `capabilities/get`.

use meerkat_core::Config;

use crate::protocol::{RpcId, RpcResponse};

/// Handle `capabilities/get` — returns the runtime's capability set with
/// status resolved against config.
pub fn handle_get(id: Option<RpcId>, config: &Config) -> RpcResponse {
    let response = meerkat::surface::build_capabilities_response(config);
    RpcResponse::success(id, &response)
}
