//! Handler for `models/catalog`.

use meerkat_core::Config;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};

/// Handle `models/catalog` using the active config-backed model registry.
pub fn handle_catalog(id: Option<RpcId>, config: &Config) -> RpcResponse {
    match meerkat::surface::build_models_catalog_response(config) {
        Ok(response) => RpcResponse::success(id, &response),
        Err(err) => RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    }
}
