//! Runtime host projection wire contracts.
//!
//! These types intentionally describe facts that a runtime surface can report.
//! They do not assign work, enroll peers, lease hosts, or decide topology.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::version::ContractVersion;

/// Scope of stability for a reported runtime host id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHostIdScope {
    /// Stable for the current runtime process only.
    Process,
    /// Derived from an explicit realm and instance pairing.
    RealmInstance,
}

/// Health state of the host process as observed by the reporting surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHostHealthStatus {
    Ok,
    Degraded,
    Unhealthy,
}

/// Existing host capability facts exposed as typed booleans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeHostFeatureFlags {
    pub runtime_backed_sessions: bool,
    pub mobs: bool,
    pub mcp_live: bool,
    pub comms: bool,
    pub blobs: bool,
    pub session_events: bool,
    pub session_streams: bool,
    pub schedules: bool,
    pub skills: bool,
    pub event_replay: bool,
    pub artifacts: bool,
    pub approvals: bool,
    pub external_members: bool,
    pub secure_remote_rpc: bool,
}

/// Realm/config metadata projected from the owning config store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeHostRealmProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_root: Option<String>,
}

/// Endpoint metadata that reports surface reachability without owning it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeHostEndpointProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rpc_methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rest_paths: Vec<String>,
}

/// Runtime capability surface for a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeHostCapabilities {
    pub contract_version: ContractVersion,
    pub features: RuntimeHostFeatureFlags,
}

/// Runtime health projection for a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeHostHealth {
    pub contract_version: ContractVersion,
    pub status: RuntimeHostHealthStatus,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, RuntimeHostHealthStatus>,
}

/// Read-only host information. This is a projection, not a registry entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeHostInfo {
    pub contract_version: ContractVersion,
    pub host_id: String,
    pub host_id_scope: RuntimeHostIdScope,
    pub process_name: String,
    pub process_version: String,
    pub capabilities: RuntimeHostCapabilities,
    pub health: RuntimeHostHealth,
    pub realm: RuntimeHostRealmProjection,
    pub endpoints: RuntimeHostEndpointProjection,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placement_labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_profile_summary: Option<String>,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn sample_flags() -> RuntimeHostFeatureFlags {
        RuntimeHostFeatureFlags {
            runtime_backed_sessions: true,
            mobs: true,
            mcp_live: true,
            comms: true,
            blobs: true,
            session_events: true,
            session_streams: true,
            schedules: true,
            skills: true,
            event_replay: false,
            artifacts: false,
            approvals: false,
            external_members: false,
            secure_remote_rpc: false,
        }
    }

    #[test]
    fn runtime_host_info_serializes_stable_host_id_scope_and_flags() {
        let info = RuntimeHostInfo {
            contract_version: ContractVersion::CURRENT,
            host_id: "realm-instance:dev:web".to_string(),
            host_id_scope: RuntimeHostIdScope::RealmInstance,
            process_name: "meerkat-rpc".to_string(),
            process_version: "0.0.0-test".to_string(),
            capabilities: RuntimeHostCapabilities {
                contract_version: ContractVersion::CURRENT,
                features: sample_flags(),
            },
            health: RuntimeHostHealth {
                contract_version: ContractVersion::CURRENT,
                status: RuntimeHostHealthStatus::Ok,
                checks: BTreeMap::new(),
            },
            realm: RuntimeHostRealmProjection {
                realm_id: Some("dev".to_string()),
                instance_id: Some("web".to_string()),
                backend: Some("sqlite".to_string()),
                state_root: Some("/tmp/meerkat".to_string()),
                context_root: None,
            },
            endpoints: RuntimeHostEndpointProjection::default(),
            placement_labels: BTreeMap::new(),
            policy_profile_summary: None,
        };

        let value = serde_json::to_value(&info).expect("serialize host info");
        assert_eq!(value["host_id"], "realm-instance:dev:web");
        assert_eq!(value["host_id_scope"], "realm_instance");
        assert_eq!(value["capabilities"]["features"]["mobs"], true);
        assert_eq!(value["capabilities"]["features"]["event_replay"], false);
    }

    #[test]
    fn runtime_host_info_does_not_claim_topology_authority() {
        let info = RuntimeHostInfo {
            contract_version: ContractVersion::CURRENT,
            host_id: "process:host-01".to_string(),
            host_id_scope: RuntimeHostIdScope::Process,
            process_name: "meerkat-rest".to_string(),
            process_version: "0.0.0-test".to_string(),
            capabilities: RuntimeHostCapabilities {
                contract_version: ContractVersion::CURRENT,
                features: sample_flags(),
            },
            health: RuntimeHostHealth {
                contract_version: ContractVersion::CURRENT,
                status: RuntimeHostHealthStatus::Ok,
                checks: BTreeMap::new(),
            },
            realm: RuntimeHostRealmProjection::default(),
            endpoints: RuntimeHostEndpointProjection::default(),
            placement_labels: BTreeMap::new(),
            policy_profile_summary: None,
        };

        let text = serde_json::to_string(&info).expect("serialize host info");
        for forbidden in ["topology", "registry", "lease", "claim", "project"] {
            assert!(
                !text.contains(forbidden),
                "host projection must not expose topology authority token `{forbidden}`: {text}"
            );
        }
    }
}
