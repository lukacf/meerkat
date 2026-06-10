//! Wire types for `config/*` write results.
//!
//! `config/set` and `config/patch` return the committed config envelope plus
//! a typed live-channel propagation report (K17): per-channel propagation
//! faults are carried in the wire result, never laundered into logs while
//! the surface reports an unqualified success.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Reason a live channel was skipped during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireLiveHotSwapSkipReason {
    NoOpOrOverride,
    IdentityLookupFailed { error: String },
}

/// A live channel that was skipped during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLiveHotSwapSkip {
    pub session_id: String,
    pub reason: WireLiveHotSwapSkipReason,
}

/// A live channel whose hot-swap failed during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLiveSwapFailure {
    pub session_id: String,
    pub error: String,
}

/// Why a live channel refresh failed during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireLiveChannelRefreshFailure {
    OpenConfigBuildFailed { error: String },
    SnapshotVersionFailed { error: String },
    EnqueueFailed { error: String },
    QueueAcceptanceRejected { error: String },
}

/// A live channel whose refresh failed during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLiveRefreshFailure {
    pub session_id: String,
    pub failure: WireLiveChannelRefreshFailure,
}

/// Why a live channel close failed during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireLiveChannelCloseFailure {
    SignalFailed { error: String },
    CloseAuthorityRejected { error: String },
    CommitHandoffMissing,
    HostCommitFailed { error: String },
}

/// A live channel whose close failed during config propagation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLiveCloseFailure {
    pub session_id: String,
    pub failure: WireLiveChannelCloseFailure,
}

/// Typed wire projection of the live-config propagation report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLiveConfigPropagationReport {
    /// `true` when every targeted channel swapped/refreshed/closed cleanly.
    pub clean: bool,
    pub swapped: Vec<String>,
    pub skipped: Vec<WireLiveHotSwapSkip>,
    pub swap_failed: Vec<WireLiveSwapFailure>,
    pub refreshed: Vec<String>,
    pub closed: Vec<String>,
    pub refresh_failed: Vec<WireLiveRefreshFailure>,
    pub close_failed: Vec<WireLiveCloseFailure>,
}

/// Result of a `config/set` or `config/patch` write.
///
/// `cfg`-gated off wasm32 alongside `meerkat_core::config_runtime`, which
/// owns the embedded envelope (the browser runtime serves no config writes).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConfigWriteResult {
    /// The committed config envelope (flattened — the write result is a
    /// strict superset of the read envelope shape).
    #[serde(flatten)]
    pub envelope: meerkat_core::ConfigEnvelope,
    /// Live-channel propagation outcome, present when the write fanned out
    /// to live channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_propagation: Option<WireLiveConfigPropagationReport>,
}

/// Parameters for the RPC `config/set` method — replace the config (bare
/// config or wrapped with an optimistic-concurrency generation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum ConfigSetParams {
    Wrapped {
        #[cfg_attr(feature = "schema", schemars(with = "Value"))]
        config: meerkat_core::Config,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_generation: Option<u64>,
    },
    Direct(#[cfg_attr(feature = "schema", schemars(with = "Value"))] meerkat_core::Config),
}

/// Parameters for the RPC `config/patch` method — RFC-7386 merge-patch
/// (bare patch or wrapped with an optimistic-concurrency generation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConfigPatchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_generation: Option<u64>,
}
