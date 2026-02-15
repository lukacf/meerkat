//! `config/*` method handlers.

use std::sync::Arc;

use serde_json::value::RawValue;
use serde_json::{Value, json};

use meerkat_core::config::{Config, ConfigDelta};
use meerkat_core::{ConfigRuntime, ConfigRuntimeError, ConfigSnapshot, ConfigStore};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};

#[derive(serde::Deserialize)]
struct ConfigSetPayload {
    #[serde(default)]
    config: Option<Config>,
    #[serde(default)]
    expected_generation: Option<u64>,
}

#[derive(serde::Deserialize)]
struct ConfigPatchPayload {
    #[serde(default)]
    patch: Option<Value>,
    #[serde(default)]
    expected_generation: Option<u64>,
}

fn config_response_body(snapshot: ConfigSnapshot) -> Value {
    let metadata = snapshot.metadata;
    json!({
        "config": snapshot.config,
        "generation": snapshot.generation,
        "realm_id": metadata.as_ref().and_then(|m| m.realm_id.clone()),
        "instance_id": metadata.as_ref().and_then(|m| m.instance_id.clone()),
        "backend": metadata.as_ref().and_then(|m| m.backend.clone()),
        "resolved_paths": metadata.as_ref().and_then(|m| m.resolved_paths.clone()),
    })
}

fn config_runtime(config_store: &Arc<dyn ConfigStore>) -> Option<ConfigRuntime> {
    ConfigRuntime::from_store_metadata(Arc::clone(config_store))
}

fn runtime_error_to_response(id: Option<RpcId>, err: ConfigRuntimeError) -> RpcResponse {
    match err {
        ConfigRuntimeError::GenerationConflict { expected, current } => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Generation conflict: expected {expected}, current {current}"),
        ),
        other => RpcResponse::error(id, error::INVALID_PARAMS, other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `config/get`.
pub async fn handle_get(id: Option<RpcId>, config_store: &Arc<dyn ConfigStore>) -> RpcResponse {
    if let Some(runtime) = config_runtime(config_store) {
        match runtime.get().await {
            Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        match config_store.get().await {
            Ok(config) => RpcResponse::success(
                id,
                config_response_body(ConfigSnapshot {
                    config,
                    generation: 0,
                    metadata: config_store.metadata(),
                }),
            ),
            Err(e) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("Failed to read config: {e}"),
            ),
        }
    }
}

/// Handle `config/set`.
pub async fn handle_set(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    config_store: &Arc<dyn ConfigStore>,
) -> RpcResponse {
    let value: Value = match parse_params(params) {
        Ok(v) => v,
        Err(resp) => return resp.with_id(id),
    };

    let (config, expected_generation) =
        if let Ok(payload) = serde_json::from_value::<ConfigSetPayload>(value.clone()) {
            match payload.config {
                Some(config) => (config, payload.expected_generation),
                None => match serde_json::from_value::<Config>(value) {
                    Ok(config) => (config, None),
                    Err(e) => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            format!("Failed to parse config payload: {e}"),
                        );
                    }
                },
            }
        } else {
            match serde_json::from_value::<Config>(value) {
                Ok(config) => (config, None),
                Err(e) => {
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        format!("Failed to parse config payload: {e}"),
                    );
                }
            }
        };

    if let Some(runtime) = config_runtime(config_store) {
        match runtime.set(config, expected_generation).await {
            Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        match config_store.set(config.clone()).await {
            Ok(()) => RpcResponse::success(
                id,
                config_response_body(ConfigSnapshot {
                    config,
                    generation: 0,
                    metadata: config_store.metadata(),
                }),
            ),
            Err(e) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Failed to set config: {e}"),
            ),
        }
    }
}

/// Handle `config/patch`.
pub async fn handle_patch(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    config_store: &Arc<dyn ConfigStore>,
) -> RpcResponse {
    let value: Value = match parse_params(params) {
        Ok(v) => v,
        Err(resp) => return resp.with_id(id),
    };
    let (patch, expected_generation) =
        if let Ok(payload) = serde_json::from_value::<ConfigPatchPayload>(value.clone()) {
            match payload.patch {
                Some(patch) => (patch, payload.expected_generation),
                None => (value, None),
            }
        } else {
            (value, None)
        };

    if let Some(runtime) = config_runtime(config_store) {
        match runtime.patch(ConfigDelta(patch), expected_generation).await {
            Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        match config_store.patch(ConfigDelta(patch)).await {
            Ok(updated) => RpcResponse::success(
                id,
                config_response_body(ConfigSnapshot {
                    config: updated,
                    generation: 0,
                    metadata: config_store.metadata(),
                }),
            ),
            Err(e) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Failed to patch config: {e}"),
            ),
        }
    }
}
