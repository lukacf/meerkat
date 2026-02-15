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

fn snapshot_from_store(config: Config, config_store: &Arc<dyn ConfigStore>) -> ConfigSnapshot {
    ConfigSnapshot {
        config,
        generation: 0,
        metadata: config_store.metadata(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `config/get`.
pub async fn handle_get(
    id: Option<RpcId>,
    config_store: &Arc<dyn ConfigStore>,
    config_runtime: Option<Arc<ConfigRuntime>>,
) -> RpcResponse {
    if let Some(runtime) = config_runtime {
        match runtime.get().await {
            Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        match config_store.get().await {
            Ok(config) => RpcResponse::success(
                id,
                config_response_body(snapshot_from_store(config, config_store)),
            ),
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}

/// Handle `config/set`.
pub async fn handle_set(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    config_store: &Arc<dyn ConfigStore>,
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

    if let Some(runtime) = config_runtime {
        match runtime.set(config, expected_generation).await {
            Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        if expected_generation.is_some() {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "expected_generation requires config runtime".to_string(),
            );
        }
        match config_store.set(config.clone()).await {
            Ok(()) => RpcResponse::success(
                id,
                config_response_body(snapshot_from_store(config, config_store)),
            ),
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}

/// Handle `config/patch`.
pub async fn handle_patch(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    config_store: &Arc<dyn ConfigStore>,
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

    if let Some(runtime) = config_runtime {
        match runtime.patch(ConfigDelta(patch), expected_generation).await {
            Ok(snapshot) => RpcResponse::success(id, config_response_body(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        if expected_generation.is_some() {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "expected_generation requires config runtime".to_string(),
            );
        }
        match config_store.patch(ConfigDelta(patch)).await {
            Ok(config) => RpcResponse::success(
                id,
                config_response_body(snapshot_from_store(config, config_store)),
            ),
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}
