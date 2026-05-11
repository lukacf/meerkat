//! Config API contract types shared by native and wasm surfaces.

use crate::config::Config;
use serde::{Deserialize, Serialize};

/// Resolved paths attached to a config store context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConfigResolvedPaths {
    pub root: String,
    pub manifest_path: String,
    pub config_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_sqlite_path: Option<String>,
    pub sessions_jsonl_dir: String,
}

/// Optional metadata for config endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConfigStoreMetadata {
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub resolved_paths: Option<ConfigResolvedPaths>,
}

/// Snapshot returned by config runtime operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConfigSnapshot {
    pub config: Config,
    pub generation: u64,
    pub metadata: Option<ConfigStoreMetadata>,
}

/// Wire envelope returned by config APIs across surfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConfigEnvelope {
    pub config: Config,
    pub generation: u64,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_paths: Option<ConfigResolvedPaths>,
}

/// Policy for exposing diagnostic filesystem paths in config envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigEnvelopePolicy {
    /// Public shape: omit resolved filesystem paths.
    Public,
    /// Diagnostic shape: include resolved filesystem paths when available.
    Diagnostic,
}

impl ConfigEnvelope {
    pub fn from_snapshot(snapshot: ConfigSnapshot, policy: ConfigEnvelopePolicy) -> Self {
        let metadata = snapshot.metadata;
        let resolved_paths = match policy {
            ConfigEnvelopePolicy::Public => None,
            ConfigEnvelopePolicy::Diagnostic => {
                metadata.as_ref().and_then(|m| m.resolved_paths.clone())
            }
        };
        Self {
            config: snapshot.config,
            generation: snapshot.generation,
            realm_id: metadata.as_ref().and_then(|m| m.realm_id.clone()),
            instance_id: metadata.as_ref().and_then(|m| m.instance_id.clone()),
            backend: metadata.as_ref().and_then(|m| m.backend.clone()),
            resolved_paths,
        }
    }
}

impl From<ConfigSnapshot> for ConfigEnvelope {
    fn from(snapshot: ConfigSnapshot) -> Self {
        Self::from_snapshot(snapshot, ConfigEnvelopePolicy::Diagnostic)
    }
}
