use meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken;
use serde::{Deserialize, Serialize};

/// Supported mob member provisioning backends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobBackendKind {
    #[default]
    #[serde(rename = "session")]
    Session,
    External,
}

impl MobBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::External => "external",
        }
    }
}

/// Concrete runtime binding for a specific member spawn.
///
/// [`MobBackendKind`] says *what kind* of backend (definition/profile level).
/// `RuntimeBinding` says *which specific runtime* (spawn/provision level).
///
/// In the identity-first mob model, stable member identity is separate from
/// runtime binding. Everything in `RuntimeBinding` is a hidden binding detail:
/// how to reach the member's runtime, not who the member is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeBinding {
    /// Member runtime is a local session managed by the session service.
    Session,
    /// Member runtime is an external process with known comms identity.
    External {
        /// Real comms peer_id of the external process (UUIDv5 derived from
        /// the Ed25519 signing pubkey).
        peer_id: String,
        /// Real comms transport address of the external process.
        address: String,
        /// One-time bootstrap proof for the initial supervisor bind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bootstrap_token: Option<BridgeBootstrapToken>,
        /// Ed25519 signing pubkey (32 bytes) of the external process.
        ///
        /// Required for peer-only trust registration; `peer_id` must derive
        /// from it. Zero-key descriptors are rejected by trust validation.
        pubkey: [u8; 32],
    },
    /// Member runtime is a session on a bound member host, materialized via
    /// the host-addressed bridge (multi-host mobs, plan §7.3).
    ///
    /// `host` is a PROJECTION of the machine's `member_placement` fact
    /// carried for dispatch; MobMachine state is authoritative and every
    /// consuming seam re-checks it (typed divergence error, never trust).
    /// Endpoint/pubkey/generation/fence/session facts are NOT carried —
    /// their single owners are the MobMachine host and identity-runtime
    /// maps (DEC-R1).
    HostMaterialized {
        host: crate::machines::mob_machine::HostId,
    },
}

impl RuntimeBinding {
    /// Returns the backend kind tag for this binding.
    ///
    /// A host-materialized member is still backed by a Meerkat session; its
    /// placement changes where that session runs, not the backend family.
    pub fn kind(&self) -> MobBackendKind {
        match self {
            Self::Session | Self::HostMaterialized { .. } => MobBackendKind::Session,
            Self::External { .. } => MobBackendKind::External,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_materialized_binding_keeps_session_backend_kind() {
        let binding = RuntimeBinding::HostMaterialized {
            host: crate::machines::mob_machine::HostId::from("host-1"),
        };
        assert_eq!(binding.kind(), MobBackendKind::Session);
    }
}
