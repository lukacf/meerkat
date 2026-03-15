//! Handler for `models/catalog`.

use crate::protocol::{RpcId, RpcResponse};

/// Handle `models/catalog` — returns the compiled-in model catalog.
pub fn handle_catalog(id: Option<RpcId>) -> RpcResponse {
    let response = meerkat::surface::build_models_catalog_response();
    RpcResponse::success(id, &response)
}
