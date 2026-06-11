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
use meerkat::session_runtime::live_orchestration::{
    LiveChannelCloseFailure, LiveChannelRefreshFailure, LiveConfigPropagationReport,
    LiveHotSwapSkipReason, should_fire_live_propagation,
};
use meerkat_contracts::wire::{
    ConfigWriteResult, WireLiveChannelCloseFailure, WireLiveChannelRefreshFailure,
    WireLiveCloseFailure, WireLiveConfigPropagationReport, WireLiveHotSwapSkip,
    WireLiveHotSwapSkipReason, WireLiveRefreshFailure, WireLiveSwapFailure,
};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// Parameters for `config/set` / `config/patch` — canonical wire contracts
/// from `meerkat-contracts` (K20); the names match the generated RPC catalog
/// descriptors and the emitted schemas.
pub use meerkat_contracts::wire::{ConfigPatchParams, ConfigSetParams};

fn config_envelope(snapshot: ConfigSnapshot) -> ConfigEnvelope {
    ConfigEnvelope::from_snapshot(snapshot, ConfigEnvelopePolicy::Diagnostic)
}

/// Project the typed live-channel propagation report into its generated wire
/// twin so `config/set` and `config/patch` responses carry the propagation
/// outcome instead of laundering per-channel faults into `tracing::warn!`
/// while the RPC reports an unqualified success.
fn wire_live_propagation_report(
    report: &LiveConfigPropagationReport,
) -> WireLiveConfigPropagationReport {
    WireLiveConfigPropagationReport {
        clean: report.is_clean(),
        swapped: report.swapped.iter().map(ToString::to_string).collect(),
        skipped: report
            .skipped
            .iter()
            .map(|(session_id, reason)| WireLiveHotSwapSkip {
                session_id: session_id.to_string(),
                reason: match reason {
                    LiveHotSwapSkipReason::NoOpOrOverride => {
                        WireLiveHotSwapSkipReason::NoOpOrOverride
                    }
                    LiveHotSwapSkipReason::IdentityLookupFailed(error) => {
                        WireLiveHotSwapSkipReason::IdentityLookupFailed {
                            error: error.clone(),
                        }
                    }
                },
            })
            .collect(),
        swap_failed: report
            .swap_failed
            .iter()
            .map(|(session_id, error)| WireLiveSwapFailure {
                session_id: session_id.to_string(),
                error: error.clone(),
            })
            .collect(),
        refreshed: report.refreshed.iter().map(ToString::to_string).collect(),
        closed: report.closed.iter().map(ToString::to_string).collect(),
        refresh_failed: report
            .refresh_failed
            .iter()
            .map(|(session_id, failure)| WireLiveRefreshFailure {
                session_id: session_id.to_string(),
                failure: match failure {
                    LiveChannelRefreshFailure::OpenConfigBuildFailed(error) => {
                        WireLiveChannelRefreshFailure::OpenConfigBuildFailed {
                            error: error.clone(),
                        }
                    }
                    LiveChannelRefreshFailure::SnapshotVersionFailed(error) => {
                        WireLiveChannelRefreshFailure::SnapshotVersionFailed {
                            error: error.clone(),
                        }
                    }
                    LiveChannelRefreshFailure::EnqueueFailed(error) => {
                        WireLiveChannelRefreshFailure::EnqueueFailed {
                            error: error.clone(),
                        }
                    }
                    LiveChannelRefreshFailure::QueueAcceptanceRejected(error) => {
                        WireLiveChannelRefreshFailure::QueueAcceptanceRejected {
                            error: error.clone(),
                        }
                    }
                },
            })
            .collect(),
        close_failed: report
            .close_failed
            .iter()
            .map(|(session_id, failure)| WireLiveCloseFailure {
                session_id: session_id.to_string(),
                failure: match failure {
                    LiveChannelCloseFailure::SignalFailed(error) => {
                        WireLiveChannelCloseFailure::SignalFailed {
                            error: error.clone(),
                        }
                    }
                    LiveChannelCloseFailure::CloseAuthorityRejected(error) => {
                        WireLiveChannelCloseFailure::CloseAuthorityRejected {
                            error: error.clone(),
                        }
                    }
                    LiveChannelCloseFailure::CommitHandoffMissing => {
                        WireLiveChannelCloseFailure::CommitHandoffMissing
                    }
                    LiveChannelCloseFailure::HostCommitFailed(error) => {
                        WireLiveChannelCloseFailure::HostCommitFailed {
                            error: error.clone(),
                        }
                    }
                },
            })
            .collect(),
    }
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

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, String> {
    // RFC-7386 merge-patch acceptance/rejection has a single owner —
    // meerkat-core. The surface only maps the typed `ConfigError` to its wire
    // shape; it does not re-implement the merge.
    meerkat_core::apply_config_patch_preview(config, patch).map_err(|e| e.to_string())
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
            Ok(snapshot) => RpcResponse::success(id, config_envelope(snapshot)),
            Err(e) => runtime_error_to_response(id, e),
        }
    } else {
        match config_store.get().await {
            Ok(config) => RpcResponse::success(
                id,
                config_envelope(snapshot_from_store(config, config_store)),
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

    let (config, expected_generation) = match serde_json::from_value::<ConfigSetParams>(value) {
        Ok(ConfigSetParams::Wrapped {
            config,
            expected_generation,
        }) => (config, expected_generation),
        Ok(ConfigSetParams::Direct(config)) => (config, None),
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
                //
                // K17: the typed propagation report is carried in the wire
                // result (`live_propagation`), not reduced to tracing — a
                // non-clean fan-out is visible to the caller.
                let live_propagation = if should_fire_live_propagation(&prior, &snapshot.config) {
                    let report = runtime.propagate_config_to_live_channels().await;
                    Some(wire_live_propagation_report(&report))
                } else {
                    None
                };
                RpcResponse::success(
                    id,
                    ConfigWriteResult {
                        envelope: config_envelope(snapshot),
                        live_propagation,
                    },
                )
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
        match config_store.set(config.clone()).await {
            Ok(()) => {
                runtime.set_skill_identity_registry(registry);
                let live_propagation = if should_fire_live_propagation(&prior, &config) {
                    let report = runtime.propagate_config_to_live_channels().await;
                    Some(wire_live_propagation_report(&report))
                } else {
                    None
                };
                RpcResponse::success(
                    id,
                    ConfigWriteResult {
                        envelope: config_envelope(snapshot_from_store(config, config_store)),
                        live_propagation,
                    },
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
        if let Ok(payload) = serde_json::from_value::<ConfigPatchParams>(value.clone()) {
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
                // R3-2-4 (P1): only fire when a propagate-affecting
                // field actually changed. Previously this fired on
                // every patch — combined with the simplified G5 rule
                // (skip iff `current_session_model == new_global_model`),
                // an unrelated tools/skills patch could retarget or
                // close live sessions whose model differed from global.
                // See `should_fire_live_propagation` for the exact
                // field set.
                //
                // K17: the typed propagation report is carried in the wire
                // result (`live_propagation`), not reduced to tracing.
                let live_propagation = if should_fire_live_propagation(&current, &snapshot.config) {
                    let report = runtime.propagate_config_to_live_channels().await;
                    Some(wire_live_propagation_report(&report))
                } else {
                    None
                };
                RpcResponse::success(
                    id,
                    ConfigWriteResult {
                        envelope: config_envelope(snapshot),
                        live_propagation,
                    },
                )
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
                //
                // R3-2-4 (P1): gate on prior-vs-new diff. See
                // `should_fire_live_propagation` for the field set.
                //
                // K17: the typed propagation report is carried in the wire
                // result (`live_propagation`), not reduced to tracing.
                let live_propagation = if should_fire_live_propagation(&current, &config) {
                    let report = runtime.propagate_config_to_live_channels().await;
                    Some(wire_live_propagation_report(&report))
                } else {
                    None
                };
                RpcResponse::success(
                    id,
                    ConfigWriteResult {
                        envelope: config_envelope(snapshot_from_store(config, config_store)),
                        live_propagation,
                    },
                )
            }
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
        }
    }
}

#[cfg(test)]
mod live_propagation_wire_tests {
    use super::*;

    /// K17 regression: a non-clean live propagation report must surface its
    /// per-channel faults typed in the wire projection — the previous handler
    /// reduced the typed report to `tracing::warn!` and returned an
    /// unqualified RPC success.
    #[test]
    fn non_clean_propagation_report_carries_typed_faults_on_the_wire() {
        let session = meerkat_core::SessionId::new();
        let mut report = LiveConfigPropagationReport::default();
        report.swapped.push(session.clone());
        report.refresh_failed.push((
            session.clone(),
            LiveChannelRefreshFailure::EnqueueFailed("channel gone".to_string()),
        ));
        report.close_failed.push((
            session.clone(),
            LiveChannelCloseFailure::CommitHandoffMissing,
        ));

        let wire = wire_live_propagation_report(&report);
        assert!(!wire.clean);
        assert_eq!(wire.swapped, vec![session.to_string()]);
        assert_eq!(wire.refresh_failed.len(), 1);
        assert_eq!(wire.refresh_failed[0].session_id, session.to_string());
        assert_eq!(
            wire.refresh_failed[0].failure,
            WireLiveChannelRefreshFailure::EnqueueFailed {
                error: "channel gone".to_string()
            }
        );
        assert_eq!(
            wire.close_failed[0].failure,
            WireLiveChannelCloseFailure::CommitHandoffMissing
        );

        // A clean report projects clean and omits fault lists.
        let clean = wire_live_propagation_report(&LiveConfigPropagationReport::default());
        assert!(clean.clean);
        assert!(clean.refresh_failed.is_empty());
        assert!(clean.close_failed.is_empty());
        assert!(clean.swap_failed.is_empty());
    }
}
