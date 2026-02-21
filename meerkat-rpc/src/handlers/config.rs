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
use crate::session_runtime::SessionRuntime;

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

fn merge_patch(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                if v.is_null() {
                    base_map.remove(&k);
                } else {
                    merge_patch(base_map.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (base_val, patch_val) => {
            *base_val = patch_val;
        }
    }
}

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, String> {
    let mut value = serde_json::to_value(config).map_err(|e| e.to_string())?;
    merge_patch(&mut value, patch);
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn build_registry_or_invalid_params(
    id: Option<RpcId>,
    config: &Config,
) -> Result<meerkat_core::skills::SourceIdentityRegistry, RpcResponse> {
    SessionRuntime::build_skill_identity_registry(config).map_err(|err| {
        RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Invalid skills source-identity configuration: {err}"),
        )
    })
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
    runtime: &Arc<SessionRuntime>,
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

    let registry = match build_registry_or_invalid_params(id.clone(), &config) {
        Ok(registry) => registry,
        Err(response) => return response,
    };

    if let Some(config_runtime) = config_runtime {
        match config_runtime.set(config, expected_generation).await {
            Ok(snapshot) => {
                runtime.set_skill_identity_registry(registry);
                RpcResponse::success(id, config_response_body(snapshot))
            }
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
            Ok(()) => {
                runtime.set_skill_identity_registry(registry);
                RpcResponse::success(
                    id,
                    config_response_body(snapshot_from_store(config, config_store)),
                )
            }
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}

/// Handle `config/patch`.
pub async fn handle_patch(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &Arc<SessionRuntime>,
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

    if let Some(config_runtime) = config_runtime {
        let current = match config_runtime.get().await {
            Ok(snapshot) => snapshot.config,
            Err(e) => return runtime_error_to_response(id, e),
        };
        let preview = match apply_patch_preview(&current, patch.clone()) {
            Ok(config) => config,
            Err(err) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Failed to apply config patch preview: {err}"),
                );
            }
        };
        let registry = match build_registry_or_invalid_params(id.clone(), &preview) {
            Ok(registry) => registry,
            Err(response) => return response,
        };

        match config_runtime
            .patch(ConfigDelta(patch), expected_generation)
            .await
        {
            Ok(snapshot) => {
                runtime.set_skill_identity_registry(registry);
                RpcResponse::success(id, config_response_body(snapshot))
            }
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
        let current = match config_store.get().await {
            Ok(config) => config,
            Err(e) => return RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        };
        let preview = match apply_patch_preview(&current, patch.clone()) {
            Ok(config) => config,
            Err(err) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Failed to apply config patch preview: {err}"),
                );
            }
        };
        let registry = match build_registry_or_invalid_params(id.clone(), &preview) {
            Ok(registry) => registry,
            Err(response) => return response,
        };
        match config_store.patch(ConfigDelta(patch)).await {
            Ok(config) => {
                runtime.set_skill_identity_registry(registry);
                RpcResponse::success(
                    id,
                    config_response_body(snapshot_from_store(config, config_store)),
                )
            }
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}
