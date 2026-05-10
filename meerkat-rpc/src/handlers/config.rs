//! `config/*` method handlers.

use std::sync::Arc;

use serde_json::Value;
use serde_json::value::RawValue;

use meerkat_core::config::{Config, ConfigDelta};
use meerkat_core::{
    ConfigEnvelope, ConfigEnvelopePolicy, ConfigRuntime, ConfigRuntimeError, ConfigSnapshot,
    ConfigStore,
};

// R3-2-4 (P1+P2): symmetric gate for live-channel propagation. See the
// helper's doc-comment for the exact field set consulted by the
// propagate body. `config/set` and `config/patch` both gate on this;
// `config/set` previously did not propagate at all (P2 under-apply),
// `config/patch` previously propagated unconditionally (P1 over-apply).
use meerkat::session_runtime::live_orchestration::should_fire_live_propagation;

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

#[allow(clippy::result_large_err)]
fn validate_config_for_runtime(
    id: Option<RpcId>,
    config: &Config,
    runtime: &SessionRuntime,
) -> Result<(), RpcResponse> {
    config.validate().map_err(|err| {
        RpcResponse::error(
            id.clone(),
            error::INVALID_PARAMS,
            format!("Invalid config: {err}"),
        )
    })?;
    build_registry_for_runtime(id, config, runtime).map(|_| ())
}

#[allow(clippy::result_large_err)]
fn build_registry_or_invalid_params(
    id: Option<RpcId>,
    config: &Config,
    context_root: Option<&std::path::Path>,
    user_root: Option<&std::path::Path>,
) -> Result<meerkat_core::skills::SourceIdentityRegistry, RpcResponse> {
    SessionRuntime::build_skill_identity_registry(config, context_root, user_root).map_err(|err| {
        RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("Invalid skills source-identity configuration: {err}"),
        )
    })
}

fn runtime_skill_identity_roots(
    runtime: &SessionRuntime,
) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    let (context_root, user_root) = runtime.skill_identity_roots();
    let default_user_root = std::env::var_os("HOME").map(std::path::PathBuf::from);
    (context_root, user_root.or(default_user_root))
}

#[allow(clippy::result_large_err)]
fn build_registry_for_runtime(
    id: Option<RpcId>,
    config: &Config,
    runtime: &SessionRuntime,
) -> Result<meerkat_core::skills::SourceIdentityRegistry, RpcResponse> {
    let (context_root, user_root) = runtime_skill_identity_roots(runtime);
    build_registry_or_invalid_params(id, config, context_root.as_deref(), user_root.as_deref())
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

    if let Err(response) = validate_config_for_runtime(id.clone(), &config, runtime) {
        return response;
    }

    let registry = match build_registry_for_runtime(id.clone(), &config, runtime) {
        Ok(registry) => registry,
        Err(response) => return response,
    };

    if let Some(config_runtime) = config_runtime {
        // R3-2-4 (P2): capture prior config BEFORE commit so the
        // post-commit propagate decision can compare prior vs. new on
        // the exact field set the orchestrator consults. See
        // `should_fire_live_propagation` for the field set.
        let prior = match config_runtime.get().await {
            Ok(snapshot) => snapshot.config,
            Err(e) => return runtime_error_to_response(id, e),
        };
        let prior_global_model = prior.agent.model.clone();
        match config_runtime.set(config, expected_generation).await {
            Ok(snapshot) => {
                let committed_registry =
                    match build_registry_for_runtime(id.clone(), &snapshot.config, runtime) {
                        Ok(registry) => registry,
                        Err(response) => {
                            let message = response
                                .error
                                .as_ref()
                                .map(|error| error.message.clone())
                                .unwrap_or_else(|| {
                                    "Committed config has invalid source-identity registry"
                                        .to_string()
                                });
                            return RpcResponse::error(id, error::INTERNAL_ERROR, message);
                        }
                    };
                runtime.set_skill_identity_registry_for_generation(
                    snapshot.generation,
                    committed_registry,
                );
                // R3-2-4 (P2): fan out to live channels iff a
                // propagate-affecting field actually changed. Previously
                // `config/set` skipped propagation entirely, so a `set`
                // that legitimately swapped `agent.model` left active
                // live sessions stale until the SDK reconnected. See
                // the helper for the exact field set.
                if should_fire_live_propagation(&prior, &snapshot.config) {
                    runtime
                        .propagate_config_to_live_channels(Some(&prior_global_model))
                        .await;
                }
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
        // R3-2-4 (P2): capture prior config BEFORE commit. Same gate
        // as the runtime branch above.
        let prior = match config_store.get().await {
            Ok(prior) => prior,
            Err(e) => return RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        };
        let prior_global_model = prior.agent.model.clone();
        match config_store.set(config.clone()).await {
            Ok(()) => {
                runtime.set_skill_identity_registry(registry);
                if should_fire_live_propagation(&prior, &config) {
                    runtime
                        .propagate_config_to_live_channels(Some(&prior_global_model))
                        .await;
                }
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
        // Capture the prior global model BEFORE committing the patch.
        // G5 revisited: the orchestrator no longer consults the prior
        // baseline (the divergence heuristic broke s72 — see
        // `should_apply_global_model_hot_swap` for the rationale), but
        // the value is still threaded through for tracing/telemetry and
        // signature stability with the rpc shim.
        let prior_global_model = current.agent.model.clone();
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
        if let Err(response) = validate_config_for_runtime(id.clone(), &preview, runtime) {
            return response;
        }

        match config_runtime
            .patch(ConfigDelta(patch), expected_generation)
            .await
        {
            Ok(snapshot) => {
                let committed_registry =
                    match build_registry_for_runtime(id.clone(), &snapshot.config, runtime) {
                        Ok(registry) => registry,
                        Err(response) => {
                            let message = response
                                .error
                                .as_ref()
                                .map(|error| error.message.clone())
                                .unwrap_or_else(|| {
                                    "Committed config has invalid source-identity registry"
                                        .to_string()
                                });
                            return RpcResponse::error(id, error::INTERNAL_ERROR, message);
                        }
                    };
                runtime.set_skill_identity_registry_for_generation(
                    snapshot.generation,
                    committed_registry,
                );
                // P1#5: a committed config patch may change the resolved
                // model/provider for sessions that already have an active
                // live channel. Propagate a `Refresh` (or `Close` if the
                // new resolution is no longer realtime-capable) command to
                // every active live channel so adapters re-seed against
                // canonical state. No-op when no live host is attached.
                //
                // G5 revisited: the prior global model is forwarded for
                // signature stability but the orchestrator's hot-swap
                // rule no longer consults it.
                //
                // R3-2-4 (P1): only fire when a propagate-affecting
                // field actually changed. Previously this fired on
                // every patch — combined with the simplified G5 rule
                // (skip iff `current_session_model == new_global_model`),
                // an unrelated tools/skills patch could retarget or
                // close live sessions whose model differed from global.
                // See `should_fire_live_propagation` for the exact
                // field set.
                if should_fire_live_propagation(&current, &snapshot.config) {
                    runtime
                        .propagate_config_to_live_channels(Some(&prior_global_model))
                        .await;
                }
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
        // Capture the prior global model BEFORE the patch. G5 revisited:
        // the orchestrator no longer consults this baseline, but it is
        // still threaded through for signature stability with the rpc
        // shim. See `should_apply_global_model_hot_swap` for the rule.
        let prior_global_model = current.agent.model.clone();
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
        if let Err(response) = validate_config_for_runtime(id.clone(), &preview, runtime) {
            return response;
        }
        let registry = match build_registry_for_runtime(id.clone(), &preview, runtime) {
            Ok(registry) => registry,
            Err(response) => return response,
        };
        match config_store.patch(ConfigDelta(patch)).await {
            Ok(config) => {
                runtime.set_skill_identity_registry(registry);
                // P1#5: propagate to live channels. See doc-comment above.
                // G5 revisited: prior_global_model is threaded for
                // signature stability only; the orchestrator's hot-swap
                // rule no longer consults it.
                //
                // R3-2-4 (P1): gate on prior-vs-new diff. See
                // `should_fire_live_propagation` for the field set.
                if should_fire_live_propagation(&current, &config) {
                    runtime
                        .propagate_config_to_live_channels(Some(&prior_global_model))
                        .await;
                }
                RpcResponse::success(
                    id,
                    config_response_body(snapshot_from_store(config, config_store)),
                )
            }
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}
