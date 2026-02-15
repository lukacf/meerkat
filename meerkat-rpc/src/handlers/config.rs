//! `config/*` method handlers.

use std::sync::Arc;

use serde_json::Value;
use serde_json::value::RawValue;

use meerkat_core::config::{Config, ConfigDelta};
use meerkat_core::{
    ConfigEnvelope, ConfigEnvelopePolicy, ConfigRuntime, ConfigRuntimeError, ConfigSnapshot,
    ConfigStore,
};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ConfigSetRequest {
    Wrapped {
        config: Config,
        #[serde(default)]
        expected_generation: Option<u64>,
    },
    Direct(Config),
}

#[derive(serde::Deserialize)]
struct ConfigPatchPayload {
    #[serde(default)]
    patch: Option<Value>,
    #[serde(default)]
    expected_generation: Option<u64>,
}

fn config_response_body(snapshot: ConfigSnapshot) -> Value {
    serde_json::to_value(ConfigEnvelope::from_snapshot(
        snapshot,
        ConfigEnvelopePolicy::Diagnostic,
    ))
    .unwrap_or_else(|err| {
        serde_json::json!({
            "error": format!("Failed to serialize config response: {err}")
        })
    })
}

fn runtime_error_to_response(id: Option<RpcId>, err: ConfigRuntimeError) -> RpcResponse {
    match err {
        ConfigRuntimeError::GenerationConflict { expected, current } => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Generation conflict: expected {expected}, current {current}"),
        ),
        other => RpcResponse::error(id, error::INTERNAL_ERROR, other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `config/get`.
pub async fn handle_get(
    id: Option<RpcId>,
    _config_store: &Arc<dyn ConfigStore>,
    config_runtime: Option<Arc<ConfigRuntime>>,
) -> RpcResponse {
    let Some(runtime) = config_runtime else {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "Config runtime unavailable".to_string(),
        );
    };
    match runtime.get().await {
        Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
        Err(e) => runtime_error_to_response(id, e),
    }
}

/// Handle `config/set`.
pub async fn handle_set(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    _config_store: &Arc<dyn ConfigStore>,
    config_runtime: Option<Arc<ConfigRuntime>>,
) -> RpcResponse {
    let value: Value = match parse_params(params) {
        Ok(v) => v,
        Err(resp) => return resp.with_id(id),
    };

    let (config, expected_generation) = match serde_json::from_value::<ConfigSetRequest>(value) {
        Ok(ConfigSetRequest::Wrapped {
            config,
            expected_generation,
        }) => (config, expected_generation),
        Ok(ConfigSetRequest::Direct(config)) => (config, None),
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Failed to parse config payload: {e}"),
            );
        }
    };

    let Some(runtime) = config_runtime else {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "Config runtime unavailable".to_string(),
        );
    };
    match runtime.set(config, expected_generation).await {
        Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
        Err(e) => runtime_error_to_response(id, e),
    }
}

/// Handle `config/patch`.
pub async fn handle_patch(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    _config_store: &Arc<dyn ConfigStore>,
    config_runtime: Option<Arc<ConfigRuntime>>,
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

    let Some(runtime) = config_runtime else {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "Config runtime unavailable".to_string(),
        );
    };
    match runtime.patch(ConfigDelta(patch), expected_generation).await {
        Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
        Err(e) => runtime_error_to_response(id, e),
    }
}
