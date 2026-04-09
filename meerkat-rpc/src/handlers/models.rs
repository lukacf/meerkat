//! Handler for `models/catalog`.

use meerkat_core::Config;

use crate::protocol::{RpcId, RpcResponse};

/// Handle `models/catalog` using the active config-backed model registry.
pub fn handle_catalog(id: Option<RpcId>, config: &Config) -> RpcResponse {
    let response = meerkat::surface::build_models_catalog_response(config);
    RpcResponse::success(id, &response)
}
