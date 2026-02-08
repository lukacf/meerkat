//! `config/*` method handlers.

use std::sync::Arc;

use serde_json::value::RawValue;

use meerkat_core::ConfigStore;
use meerkat_core::config::{Config, ConfigDelta};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `config/get`.
pub async fn handle_get(id: Option<RpcId>, config_store: &Arc<dyn ConfigStore>) -> RpcResponse {
    match config_store.get().await {
        Ok(config) => RpcResponse::success(id, config),
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("Failed to read config: {e}"),
        ),
    }
}

/// Handle `config/set`.
pub async fn handle_set(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    config_store: &Arc<dyn ConfigStore>,
) -> RpcResponse {
    let config: Config = match parse_params(params) {
        Ok(c) => c,
        Err(resp) => return resp.with_id(id),
    };

    match config_store.set(config).await {
        Ok(()) => RpcResponse::success(id, serde_json::json!({"ok": true})),
        Err(e) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Failed to set config: {e}"),
        ),
    }
}

/// Handle `config/patch`.
pub async fn handle_patch(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    config_store: &Arc<dyn ConfigStore>,
) -> RpcResponse {
    let delta: ConfigDelta = match parse_params(params) {
        Ok(d) => d,
        Err(resp) => return resp.with_id(id),
    };

    match config_store.patch(delta).await {
        Ok(updated) => RpcResponse::success(id, updated),
        Err(e) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Failed to patch config: {e}"),
        ),
    }
}
