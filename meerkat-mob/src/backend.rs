use meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken;
use serde::{Deserialize, Serialize};

/// Supported mob member provisioning backends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
        /// Required for peer-only trust registration. Legacy callers may
        /// deserialize with this field absent, but live wiring and bridge
        /// control paths fail closed instead of installing a zero-pubkey
        /// trust descriptor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pubkey: Option<[u8; 32]>,
    },
}

impl RuntimeBinding {
    /// Returns the backend kind tag for this binding.
    pub fn kind(&self) -> MobBackendKind {
        match self {
            Self::Session => MobBackendKind::Session,
            Self::External { .. } => MobBackendKind::External,
        }
    }
}
