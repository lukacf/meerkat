//! Core comms configuration types.

use crate::CommsConfig;
use meerkat_core::CommsAuthMode;
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::net::SocketAddr;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

/// Core configuration for agent-to-agent communication.
///
/// The `listen_tcp`/`listen_uds` addresses are for signed (CBOR+Ed25519)
/// agent-to-agent communication. The `event_listen_tcp`/`event_listen_uds`
/// addresses are for the plain-text external event listener (when `auth=Open`).
///
/// Both listeners can run simultaneously — the signed listener handles
/// peer agents, while the plain listener accepts external events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoreCommsConfig {
    pub enabled: bool,
    pub name: String,
    /// Optional namespace for in-process registry isolation.
    ///
    /// Agents in different namespaces cannot see or send to each other via
    /// `inproc://` unless they explicitly share this value.
    pub inproc_namespace: Option<String>,
    /// Address for signed (Ed25519) agent-to-agent listener.
    #[cfg(not(target_arch = "wasm32"))]
    pub listen_uds: Option<PathBuf>,
    /// Address for signed (Ed25519) agent-to-agent listener.
    #[cfg(not(target_arch = "wasm32"))]
    pub listen_tcp: Option<SocketAddr>,
    /// Address for plain-text external event listener. Only active when `auth=Open`.
    #[cfg(not(target_arch = "wasm32"))]
    pub event_listen_tcp: Option<SocketAddr>,
    /// Path for plain-text external event listener (UDS). Only active when `auth=Open`.
    #[cfg(unix)]
    pub event_listen_uds: Option<PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    pub identity_dir: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    pub trusted_peers_path: PathBuf,
    pub ack_timeout_secs: u64,
    pub max_message_bytes: u32,
    pub auth: CommsAuthMode,
    pub require_peer_auth: bool,
    /// Allow binding plain event listener to non-loopback addresses.
    /// This is a prompt injection vector — only enable with explicit intent.
    pub allow_external_unauthenticated: bool,
}

impl Default for CoreCommsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: "meerkat".to_string(),
            inproc_namespace: None,
            #[cfg(not(target_arch = "wasm32"))]
            listen_uds: None,
            #[cfg(not(target_arch = "wasm32"))]
            listen_tcp: None,
            #[cfg(not(target_arch = "wasm32"))]
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            #[cfg(not(target_arch = "wasm32"))]
            identity_dir: PathBuf::from(".rkat/identity"),
            #[cfg(not(target_arch = "wasm32"))]
            trusted_peers_path: PathBuf::from(".rkat/trusted_peers.json"),
            ack_timeout_secs: 30,
            max_message_bytes: 1_048_576,
            auth: CommsAuthMode::default(),
            require_peer_auth: true,
            allow_external_unauthenticated: false,
        }
    }
}

impl CoreCommsConfig {
    pub fn with_name(name: &str) -> Self {
        Self {
            enabled: true,
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn interpolate_path(&self, path: &Path) -> PathBuf {
        let path_str = path.to_string_lossy();
        let interpolated = path_str.replace("{name}", &self.name);
        PathBuf::from(interpolated)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve_paths(&self, base_dir: &Path) -> ResolvedCommsConfig {
        let resolve = |path: &Path| -> PathBuf {
            let interpolated = self.interpolate_path(path);
            if interpolated.is_absolute() {
                interpolated
            } else {
                base_dir.join(interpolated)
            }
        };

        ResolvedCommsConfig {
            enabled: self.enabled,
            name: self.name.clone(),
            inproc_namespace: self.inproc_namespace.clone(),
            listen_uds: self.listen_uds.as_ref().map(|p| resolve(p)),
            listen_tcp: self.listen_tcp,
            event_listen_tcp: self.event_listen_tcp,
            #[cfg(unix)]
            event_listen_uds: self.event_listen_uds.as_ref().map(|p| resolve(p)),
            identity_dir: resolve(&self.identity_dir),
            trusted_peers_path: resolve(&self.trusted_peers_path),
            comms_config: CommsConfig {
                ack_timeout_secs: self.ack_timeout_secs,
                max_message_bytes: self.max_message_bytes,
            },
            auth: self.auth,
            require_peer_auth: self.require_peer_auth,
            allow_external_unauthenticated: self.allow_external_unauthenticated,
        }
    }
}

/// Resolved comms configuration with absolute paths.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCommsConfig {
    pub enabled: bool,
    pub name: String,
    pub inproc_namespace: Option<String>,
    /// Address for signed (Ed25519) agent-to-agent listener.
    pub listen_uds: Option<PathBuf>,
    /// Address for signed (Ed25519) agent-to-agent listener.
    pub listen_tcp: Option<SocketAddr>,
    /// Address for plain-text external event listener.
    pub event_listen_tcp: Option<SocketAddr>,
    /// Path for plain-text external event listener (UDS).
    #[cfg(unix)]
    pub event_listen_uds: Option<PathBuf>,
    pub identity_dir: PathBuf,
    pub trusted_peers_path: PathBuf,
    pub comms_config: CommsConfig,
    pub auth: CommsAuthMode,
    pub require_peer_auth: bool,
    pub allow_external_unauthenticated: bool,
}
