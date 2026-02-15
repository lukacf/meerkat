//! `config/*` method handlers.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::value::RawValue;
use serde_json::{Value, json};

use meerkat_core::ConfigStore;
use meerkat_core::config::{Config, ConfigDelta};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};

fn generation_counter() -> &'static AtomicU64 {
    static GEN: OnceLock<AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| AtomicU64::new(0))
}

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

fn config_response_body(config: Config, config_store: &Arc<dyn ConfigStore>) -> Value {
    let generation = generation_counter().load(Ordering::SeqCst);
    let metadata = config_store.metadata();
    json!({
        "config": config,
        "generation": generation,
        "realm_id": metadata.as_ref().and_then(|m| m.realm_id.clone()),
        "instance_id": metadata.as_ref().and_then(|m| m.instance_id.clone()),
        "backend": metadata.as_ref().and_then(|m| m.backend.clone()),
        "resolved_paths": metadata.as_ref().and_then(|m| m.resolved_paths.clone()),
    })
}

fn check_generation(expected_generation: Option<u64>) -> Result<(), String> {
    if let Some(expected) = expected_generation {
        let current = generation_counter().load(Ordering::SeqCst);
        if expected != current {
            return Err(format!(
                "Generation conflict: expected {expected}, current {current}"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handle `config/get`.
pub async fn handle_get(id: Option<RpcId>, config_store: &Arc<dyn ConfigStore>) -> RpcResponse {
    match config_store.get().await {
        Ok(config) => RpcResponse::success(id, config_response_body(config, config_store)),
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

    if let Err(msg) = check_generation(expected_generation) {
        return RpcResponse::error(id, error::INVALID_PARAMS, msg);
    }

    match config_store.set(config.clone()).await {
        Ok(()) => {
            generation_counter().fetch_add(1, Ordering::SeqCst);
            RpcResponse::success(id, config_response_body(config, config_store))
        }
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

    if let Err(msg) = check_generation(expected_generation) {
        return RpcResponse::error(id, error::INVALID_PARAMS, msg);
    }

    match config_store.patch(ConfigDelta(patch)).await {
        Ok(updated) => {
            generation_counter().fetch_add(1, Ordering::SeqCst);
            RpcResponse::success(id, config_response_body(updated, config_store))
        }
        Err(e) => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Failed to patch config: {e}"),
        ),
    }
}
