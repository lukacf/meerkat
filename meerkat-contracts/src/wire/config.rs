//! Wire types for `config/*` write results.
//!
//! `config/set` and `config/patch` return the committed config envelope plus
//! a typed live-channel propagation report (K17): per-channel propagation
//! faults are carried in the wire result, never laundered into logs while
//! the surface reports an unqualified success.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Schema-only projection for config write payloads.
///
/// The runtime [`meerkat_core::Config`] type owns defaults, loading, and
/// platform-dependent behavior. Public config-set contracts need only the
/// stable section layout, so schema emission points here while serde continues
/// to deserialize into the real runtime type.
#[cfg(feature = "schema")]
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
#[allow(dead_code)]
pub struct ConfigContractSchema {
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub agent: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub storage: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub budget: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub retry: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tools: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub models: Value,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    #[schemars(range(min = 1))]
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub shell: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub store: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub comms: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub compaction: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub limits: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub rest: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub hooks: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub skills: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub self_hosted: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub provider_tools: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub model_fallback: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub presentation: Value,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub realm: std::collections::BTreeMap<String, Value>,
}

#[cfg(feature = "schema")]
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

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
        #[cfg_attr(feature = "schema", schemars(with = "ConfigContractSchema"))]
        config: meerkat_core::Config,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_generation: Option<u64>,
    },
    Direct(
        #[cfg_attr(feature = "schema", schemars(with = "ConfigContractSchema"))]
        meerkat_core::Config,
    ),
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

#[cfg(all(test, feature = "schema"))]
mod tests {
    use super::*;

    #[test]
    fn config_contract_schema_matches_max_tokens_validation_floor()
    -> Result<(), Box<dyn std::error::Error>> {
        let schema = schemars::schema_for!(ConfigContractSchema);
        let schema_json = serde_json::to_value(schema)?;
        assert_eq!(
            schema_json
                .pointer("/properties/max_tokens/minimum")
                .and_then(serde_json::Value::as_u64),
            Some(1),
            "Config::validate rejects max_tokens == 0, so the public config-set schema must not advertise zero as valid"
        );
        Ok(())
    }
}
